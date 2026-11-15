#!/usr/bin/env bash
# Audit hors ligne de l'historique de chat Steam.
#
# Ce script ne lance pas le client Steam, ne se connecte à aucun service et
# n'envoie aucun message. Il exécute les tests du contrat
# `FriendMessages.GetRecentMessages#1`, puis compte les messages de l'archive
# locale sans afficher leur texte ni les identifiants des contacts.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

archive="${1:-${LAPINUX_CHAT_ARCHIVE:-${XDG_DATA_HOME:-$HOME/.local/share}/lapinux/steam-chats.json}}"

echo "=== Contrat FriendMessages.GetRecentMessages#1 (hors ligne) ==="
echo "Source : crates/steam/src/social.rs"
echo "Méthode : $(grep -m1 'const FRIEND_MESSAGES_RECENT' crates/steam/src/social.rs)"
echo "Page demandée : $(grep -m1 'const HISTORY_MESSAGES' crates/steam/src/social.rs)"
echo "Borne basse : $(grep -m1 'const HISTORY_START_TIME' crates/steam/src/social.rs)"
echo
cargo test -p lapinux-steam --lib historique -- --nocapture
echo
echo "Les cinq tests ci-dessus vérifient les champs 1/2/3/5 de la requête,"
echo "les curseurs 8/9 de la page précédente, le sens des auteurs et la"
echo "déduplication : ils ne contactent ni Steam ni le client Steam."

echo
echo "=== Archive locale : $(printf '%s' "$archive") ==="
if [[ ! -f "$archive" ]]; then
    echo "Archive absente : aucun message local à auditer."
    echo "Le contrat protocolaire ci-dessus reste vérifié ; l'absence d'archive"
    echo "n'est pas une preuve d'historique complet."
else
    python3 - "$archive" <<'PY'
import datetime as dt
import json
import os
import sys
import time

path = sys.argv[1]
try:
    with open(path, encoding="utf-8") as source:
        data = json.load(source)
except (OSError, ValueError) as error:
    print(f"Archive illisible : {error}")
    raise SystemExit(1)

if not isinstance(data, list):
    print("Archive illisible : la racine n'est pas une liste de conversations.")
    raise SystemExit(1)

now = int(time.time())
retention = 14 * 86400
total = incoming = outgoing = old = 0
for index, entry in enumerate(data):
    if not isinstance(entry, list) or len(entry) != 2 or not isinstance(entry[1], list):
        print(f"Conversation {index} ignorée : format inattendu.")
        continue
    messages = [message for message in entry[1] if isinstance(message, dict)]
    timestamps = [
        int(message["at"])
        for message in messages
        if isinstance(message.get("at"), (int, float)) and message["at"] > 0
    ]
    sent = sum(1 for message in messages if message.get("outgoing") is True)
    received = len(messages) - sent
    total += len(messages)
    outgoing += sent
    incoming += received
    old += sum(1 for timestamp in timestamps if now - timestamp > retention)
    bounds = ""
    if timestamps:
        oldest = dt.datetime.fromtimestamp(min(timestamps), dt.timezone.utc)
        newest = dt.datetime.fromtimestamp(max(timestamps), dt.timezone.utc)
        bounds = (
            f", du {oldest.isoformat()} au {newest.isoformat()}"
        )
    print(
        f"Conversation {index} : {len(messages)} message(s), "
        f"{sent} sortant(s), {received} entrant(s){bounds}."
    )

print(f"Total : {total} message(s), {outgoing} sortant(s), {incoming} entrant(s).")
print(f"Messages de plus de 14 jours encore présents localement : {old}.")
print(
    "Cette archive ne contient que ce que Lapinux a déjà reçu ou importé ; "
    "elle ne prouve pas que Steam a rendu l'intégralité de la conversation."
)
if incoming == 0 and total:
    print(
        "Aucun message entrant : l'archive ne peut pas représenter une "
        "conversation complète à elle seule."
    )
PY
fi

echo
echo "=== Conclusion de la sonde ==="
echo "Fenêtre documentée par Valve : jusqu'à deux semaines de chat et de groupes."
echo "Aucun client Steam n'a été lancé, aucune capture n'a été prise, aucun"
echo "message n'a été envoyé et aucune entrée clavier/souris n'a été utilisée."
echo "Verdict : NON, pas en totalité via le protocole ; voir"
echo "docs/RECHERCHE-HISTORIQUE-CHAT-STEAM.md pour les limites exactes."
