#!/usr/bin/env python3
"""Lecteur de bande-annonce pour Lapinux.

GStreamer décode le manifeste DASH ou HLS, l'audio sort par la sortie sonore
du bureau et les images sont envoyées à l'interface, qui les peint dans son
propre panneau. Aucun navigateur, aucune fenêtre vidéo séparée.

Usage : trailer_helper.py <adresse>

Sortie standard, une trame à la fois :

  F <largeur> <hauteur> <octets>\\n  puis les pixels RGBA, sans remplissage
  E <json>\\n

Entrée standard : une ligne JSON par commande.

  {"cmd":"play"} {"cmd":"pause"} {"cmd":"seek","value":0.0..1.0}
  {"cmd":"mute","value":true|false} {"cmd":"volume","value":0.0..1.0}
  {"cmd":"quit"}

Deux variables d'environnement servent aux essais hors interface :
LAPINUX_TRAILER_SILENT=1 coupe le son et LAPINUX_TRAILER_LIMIT=N arrête le
lecteur après N images.
"""

import json
import os
import sys
import threading

import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402

Gst.init(None)

WIDTH = 960
HEIGHT = 540
FRAME_BYTES = WIDTH * HEIGHT * 4
OUT = sys.stdout.buffer
LOCK = threading.Lock()
SILENT = os.environ.get("LAPINUX_TRAILER_SILENT") == "1"
LIMIT = int(os.environ.get("LAPINUX_TRAILER_LIMIT") or "0")

state = {
    "player": None,
    "loop": None,
    "ended": False,
    "muted": False,
    "volume": 0.85,
    "frames": 0,
    "bad_size": False,
    "stopping": False,
    "error": False,
}


def emit(payload):
    """Une ligne JSON, jamais au milieu d'une image."""
    data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    with LOCK:
        OUT.write(b"E " + data + b"\n")
        OUT.flush()


def stop():
    if state["stopping"]:
        return False
    state["stopping"] = True
    player = state["player"]
    if player is not None:
        player.set_state(Gst.State.NULL)
    loop = state["loop"]
    if loop is not None:
        loop.quit()
    return False


def make_video_sink():
    """Le bac d'images : conversion RGBA à taille fixe, puis appsink.

    La largeur 960 est un multiple de 128 : les lignes n'ont pas de
    remplissage, l'interface reçoit exactement 960 × 540 × 4 octets.
    """
    bin_ = Gst.Bin.new("lapinux-video")
    convert = Gst.ElementFactory.make("videoconvert", None)
    rate = Gst.ElementFactory.make("videorate", None)
    scale = Gst.ElementFactory.make("videoscale", None)
    if scale is not None:
        try:
            scale.set_property("add-borders", True)
        except TypeError:
            pass
    caps = Gst.ElementFactory.make("capsfilter", None)
    caps.set_property(
        "caps",
        Gst.Caps.from_string(
            "video/x-raw,format=RGBA,width=960,height=540,"
            "framerate=30/1,pixel-aspect-ratio=1/1"
        ),
    )
    sink = Gst.ElementFactory.make("appsink", None)
    sink.set_property("emit-signals", True)
    sink.set_property("max-buffers", 1)
    sink.set_property("drop", True)
    # sync=True : les images suivent l'horloge de la bande-son au lieu
    # d'arriver aussi vite que le décodeur le permet.
    sink.set_property("sync", True)
    sink.connect("new-sample", on_sample)

    for element in (convert, rate, scale, caps, sink):
        if element is not None:
            bin_.add(element)
    convert.link(rate)
    rate.link(scale)
    scale.link(caps)
    caps.link(sink)
    bin_.add_pad(Gst.GhostPad.new("sink", convert.get_static_pad("sink")))
    return bin_


def on_sample(sink):
    sample = sink.emit("pull-sample")
    if sample is None:
        return Gst.FlowReturn.OK
    buffer = sample.get_buffer()
    ok, info = buffer.map(Gst.MapFlags.READ)
    if not ok:
        return Gst.FlowReturn.OK
    try:
        data = info.data
        size = len(data)
        if size != FRAME_BYTES:
            if not state["bad_size"]:
                state["bad_size"] = True
                emit(
                    {
                        "event": "error",
                        "message": "Format vidéo inattendu (%d octets)." % size,
                    }
                )
            return Gst.FlowReturn.OK
        with LOCK:
            OUT.write(b"F %d %d %d\n" % (WIDTH, HEIGHT, size))
            OUT.write(data)
            OUT.flush()
        state["frames"] += 1
        if LIMIT and state["frames"] >= LIMIT:
            GLib.idle_add(stop)
    finally:
        buffer.unmap(info)
    return Gst.FlowReturn.OK


def on_message(_bus, message):
    if message.type == Gst.MessageType.ERROR:
        error, debug = message.parse_error()
        state["error"] = True
        emit({"event": "error", "message": error.message})
        player = state["player"]
        if player is not None:
            player.set_state(Gst.State.PAUSED)
    elif message.type == Gst.MessageType.EOS:
        state["ended"] = True
        emit({"event": "ended"})
        player = state["player"]
        if player is not None:
            player.set_state(Gst.State.PAUSED)
    elif message.type == Gst.MessageType.BUFFERING:
        emit({"event": "buffering", "percent": message.parse_buffering()})
    return True


def tick():
    if state["stopping"]:
        return False
    player = state["player"]
    if player is None:
        return True
    _, current, _ = player.get_state(0)
    pos_ok, pos = player.query_position(Gst.Format.TIME)
    dur_ok, dur = player.query_duration(Gst.Format.TIME)
    emit(
        {
            "event": "time",
            "position": (pos / 1_000_000_000) if pos_ok else 0.0,
            "duration": (dur / 1_000_000_000) if dur_ok else 0.0,
            "playing": current == Gst.State.PLAYING,
        }
    )
    return True


def set_playing(play):
    player = state["player"]
    if player is None or state["error"]:
        return
    if play:
        if state["ended"]:
            player.seek_simple(
                Gst.Format.TIME,
                Gst.SeekFlags.FLUSH | Gst.SeekFlags.KEY_UNIT,
                0,
            )
            state["ended"] = False
        player.set_state(Gst.State.PLAYING)
    else:
        player.set_state(Gst.State.PAUSED)


def on_command(message):
    player = state["player"]
    if player is None:
        return False
    command = message.get("cmd")
    if command == "play":
        set_playing(True)
    elif command == "pause":
        set_playing(False)
    elif command == "seek":
        try:
            value = max(0.0, min(1.0, float(message.get("value", 0.0))))
        except (TypeError, ValueError):
            value = 0.0
        ok, duration = player.query_duration(Gst.Format.TIME)
        if ok and duration > 0:
            state["ended"] = False
            player.seek_simple(
                Gst.Format.TIME,
                Gst.SeekFlags.FLUSH | Gst.SeekFlags.KEY_UNIT,
                int(duration * value),
            )
    elif command == "mute":
        state["muted"] = bool(message.get("value", True))
        player.set_property(
            "volume", 0.0 if state["muted"] else state["volume"]
        )
    elif command == "volume":
        try:
            state["volume"] = max(
                0.0, min(1.0, float(message.get("value", 0.85)))
            )
        except (TypeError, ValueError):
            state["volume"] = 0.85
        state["muted"] = False
        player.set_property("volume", state["volume"])
    elif command == "quit":
        stop()
    return False


def read_commands():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except ValueError:
            continue
        GLib.idle_add(on_command, message)
    GLib.idle_add(stop)


def main():
    url = sys.argv[1] if len(sys.argv) > 1 else ""
    if not url:
        emit({"event": "error", "message": "Aucune adresse de bande-annonce."})
        return 1

    player = Gst.ElementFactory.make("playbin", "lapinux-trailer")
    if player is None:
        emit({"event": "error", "message": "GStreamer est indisponible."})
        return 1
    state["player"] = player
    player.set_property("uri", url)
    player.set_property("video-sink", make_video_sink())
    player.set_property("volume", state["volume"])
    if SILENT:
        player.set_property("audio-sink", Gst.ElementFactory.make("fakesink", None))

    bus = player.get_bus()
    bus.add_signal_watch()
    bus.connect("message", on_message)

    loop = GLib.MainLoop()
    state["loop"] = loop
    threading.Thread(target=read_commands, daemon=True).start()
    GLib.timeout_add(250, tick)
    emit({"event": "ready"})
    player.set_state(Gst.State.PLAYING)
    loop.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
