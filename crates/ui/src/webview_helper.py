#!/usr/bin/env python3
"""Fenêtre de boutique pour Lapinux.

Lapinux n'est pas un navigateur : pour montrer le site d'une boutique, il
confie la page à **WebKitGTK 6**, le moteur de GNOME Web, dans une fenêtre à
part. La page y est rendue comme dans un navigateur — défilement au pixel,
animations, clavier, liens, formulaires — parce que c'en est un : aucune capture
d'écran ni transmission d'images entre les deux processus.

La pile est celle de GTK 4 et WebKitGTK 6, natives sous Wayland : l'application
reste native, aucun XWayland n'est nécessaire.

La fenêtre est autonome : la fermer arrête ce processus, et Lapinux le voit
(reçu `{"event":"closed"}`) sans rien avoir à faire.

Usage : webview_helper.py <titre> <adresse>

Protocole : une ligne JSON par message, sur l'entrée et la sortie standard.

  entrant  {"cmd":"load","url":"https://…","title":"Steam"}
           {"cmd":"back"} {"cmd":"reload"} {"cmd":"present"} {"cmd":"quit"}
  sortant  {"event":"ready"} {"event":"closed"}
           {"event":"loading","value":true|false} {"event":"url","url":…}
"""

import json
import os
import sys
import threading

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Soup", "3.0")
gi.require_version("WebKit", "6.0")
from gi.repository import Gio, GLib, Gtk, Soup, WebKit  # noqa: E402

TITLE = sys.argv[1] if len(sys.argv) > 1 else "Boutique"
URL = sys.argv[2] if len(sys.argv) > 2 else ""

state = {"window": None, "view": None, "app": None}

# Les cookies datés sont écrits par WebKit ; ceux de session, non. Or c'est
# souvent la connexion elle-même qui n'a pas d'échéance : elle était donc
# perdue à chaque fermeture. On les recopie donc nous-mêmes, avec une date
# lointaine, dans un petit fichier relu au lancement suivant.
COOKIES = {"manager": None, "file": ""}
SESSION_LIFE = 30 * 24 * 3600


def emit(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def build(application):
    # Un agent de navigateur de bureau : sans lui, certains sites servent un mur
    # de vérification « êtes-vous un robot ? » à la place de la page.
    settings = WebKit.Settings()
    settings.set_user_agent(
        "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"
    )
    settings.set_enable_smooth_scrolling(True)
    # Les vidéos se comportent comme dans un navigateur : plus de blocage
    # d'autoplay ni de pause des lecteurs lancés par la page. Le bandeau
    # d'accueil Steam peut donc tourner en boucle et coûter un cœur entier.
    settings.set_media_playback_requires_user_gesture(False)

    window = Gtk.ApplicationWindow(application=application)
    window.set_title(TITLE)
    window.set_default_size(1280, 860)
    window.set_icon_name("lapinux")

    # Les témoins de connexion vivent sur le disque : une fois connecté chez
    # Epic dans cette fenêtre, le passage en caisse d'une offre gratuite se
    # termine sans redemander d'identifiants à chaque ouverture.
    data = os.path.join(GLib.get_user_data_dir(), "lapinux", "store-web")
    session = WebKit.NetworkSession(
        data_directory=os.path.join(data, "data"),
        cache_directory=os.path.join(data, "cache"),
        is_ephemeral=False,
    )
    # Sans stockage persistant explicite, WebKit garde les cookies en mémoire :
    # la connexion à une boutique et le panier disparaissaient dès que la
    # fenêtre se fermait, et il fallait se reconnecter à chaque fois.
    cookies = session.get_cookie_manager()
    cookies.set_persistent_storage(
        os.path.join(data, "cookies.sqlite"), WebKit.CookiePersistentStorage.SQLITE
    )
    cookies.set_accept_policy(WebKit.CookieAcceptPolicy.NO_THIRD_PARTY)
    COOKIES["manager"] = cookies
    COOKIES["file"] = os.path.join(data, "cookies-session.json")
    restore_session_cookies()

    view = WebKit.WebView(network_session=session, settings=settings)
    window.set_child(view)
    window.present()

    view.connect("load-changed", on_load_changed)
    window.connect("close-request", on_close)

    state["window"] = window
    state["view"] = view
    view.load_uri(URL)


def restore_session_cookies():
    """Remet les cookies de session d'une visite précédente, datés au passage."""
    manager = COOKIES["manager"]
    if manager is None or not COOKIES["file"]:
        return
    try:
        with open(COOKIES["file"], encoding="utf-8") as handle:
            entries = json.load(handle)
    except (OSError, ValueError):
        return
    for entry in entries:
        try:
            cookie = Soup.Cookie.new(
                entry["name"], entry["value"], entry["domain"], entry["path"], -1
            )
            cookie.set_max_age(SESSION_LIFE)
            cookie.set_secure(bool(entry.get("secure")))
            cookie.set_http_only(bool(entry.get("http_only")))
            manager.add_cookie(cookie)
        except Exception:  # noqa: BLE001
            continue


def save_session_cookies(after=None):
    """Écrit les cookies de session avant de s'arrêter, puis continue."""
    manager = COOKIES["manager"]
    if manager is None or not COOKIES["file"]:
        if after is not None:
            after()
        return False

    def written(manager_, result, _data=None):
        entries = []
        try:
            for cookie in manager_.get_all_cookies_finish(result):
                if cookie.get_expires() is not None:
                    continue
                entries.append(
                    {
                        "name": cookie.get_name(),
                        "value": cookie.get_value(),
                        "domain": cookie.get_domain(),
                        "path": cookie.get_path() or "/",
                        "secure": cookie.get_secure(),
                        "http_only": cookie.get_http_only(),
                    }
                )
        except Exception:  # noqa: BLE001
            entries = []
        try:
            with open(COOKIES["file"], "w", encoding="utf-8") as handle:
                json.dump(entries, handle)
        except OSError:
            pass
        if after is not None:
            after()
        return False

    try:
        manager.get_all_cookies(None, written, None)
    except Exception:  # noqa: BLE001
        if after is not None:
            after()
    return False


def on_close(_window):
    emit({"event": "closed"})
    if state["app"] is not None:
        save_session_cookies(state["app"].quit)
    return False


def on_load_changed(_view, event):
    if event == WebKit.LoadEvent.STARTED:
        emit({"event": "loading", "value": True})
    elif event == WebKit.LoadEvent.FINISHED:
        view = state["view"]
        emit({"event": "url", "url": view.get_uri() or "" if view else ""})
        emit({"event": "loading", "value": False})
    return False


def on_command(message):
    view = state["view"]
    window = state["window"]
    command = message.get("cmd")
    if command == "load" and view is not None:
        view.load_uri(message.get("url", ""))
        title = message.get("title")
        if title and window is not None:
            window.set_title(title)
        if window is not None:
            window.present()
    elif command == "title" and window is not None:
        window.set_title(message.get("value", TITLE))
    elif command == "back" and view is not None:
        if view.can_go_back():
            view.go_back()
    elif command == "reload" and view is not None:
        view.reload()
    elif command == "present" and window is not None:
        window.present()
    elif command == "quit" and state["app"] is not None:
        save_session_cookies(state["app"].quit)
        return False
    return False


def read_commands():
    """Lit les commandes hors du fil GTK, et les y ramène une par une."""
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except Exception:
            continue
        GLib.idle_add(on_command, message)
    GLib.idle_add(on_command, {"cmd": "quit"})


# `NON_UNIQUE` : chaque boutique a sa propre instance, sinon une seconde
# ouverture se contenterait de réveiller la première.
app = Gtk.Application(
    application_id="org.lapinux.boutique",
    flags=Gio.ApplicationFlags.NON_UNIQUE,
)
state["app"] = app
app.connect("activate", build)
threading.Thread(target=read_commands, daemon=True).start()
emit({"event": "ready"})
app.run([])
