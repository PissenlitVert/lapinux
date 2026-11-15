#!/usr/bin/env python3
"""Remplit le panier Epic sans ouvrir de fenêtre.

Les cookies de la boutique sont écrits sur le disque par la fenêtre WebKit
(`cookies.sqlite`) : une fois connecté, le panier répond donc directement, sans
navigateur à afficher. Chaque offre est une mutation GraphQL, et le résultat est
rendu sur une ligne JSON que l'interface lit pour l'annoncer dans son modal.

Usage : cart_helper.py <cookies.sqlite> <espace:offre>…

Sortie : {"added": 2, "errors": [], "notLoggedIn": false, "done": true}
"""

import json
import sqlite3
import sys
import urllib.error
import urllib.request

AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"
)
URL = "https://store.epicgames.com/graphql"
QUERY = (
    "mutation AddToCart($namespace: String!, $offerId: String!) "
    "{ Cart { addToCart(namespace: $namespace, offerId: $offerId) { __typename } } }"
)


def cookies(path):
    """Les témoins Epic tels que la fenêtre de boutique les a laissés."""
    try:
        base = sqlite3.connect("file:%s?mode=ro" % path, uri=True)
        rows = base.execute(
            "select name, value from moz_cookies where host like '%epicgames%'"
        ).fetchall()
        base.close()
    except Exception:  # noqa: BLE001 - base absente ou illisible : pas de session
        return ""
    return "; ".join("%s=%s" % (name, value) for name, value in rows)


def add(jar, product):
    """Rend `ok`, `refus` ou `inconnu` pour une offre."""
    cut = product.find(":")
    if cut <= 0:
        return "refus", ""
    body = json.dumps(
        {
            "query": QUERY,
            "variables": {"namespace": product[:cut], "offerId": product[cut + 1 :]},
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        URL,
        data=body,
        headers={
            "Content-Type": "application/json",
            "Cookie": jar,
            "User-Agent": AGENT,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as reply:
            text = reply.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as error:
        text = error.read().decode("utf-8", "replace") if error.fp else ""
        # Un refus de la boutique se lit aussi dans le corps : la mutation
        # répond parfois 200 avec des erreurs GraphQL.
        return ("inconnu", text) if "notLoggedIn" in text else ("refus", text)
    except Exception as error:  # noqa: BLE001 - réseau, délai, certificat
        return "refus", str(error)
    if "notLoggedIn" in text:
        return "inconnu", text
    return ("refus", text) if '"errors"' in text else ("ok", text)


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else ""
    products = sys.argv[2:]
    jar = cookies(base)
    state = {"added": 0, "errors": [], "notLoggedIn": not jar, "done": True}
    if jar:
        for product in products:
            verdict, _detail = add(jar, product)
            if verdict == "ok":
                state["added"] += 1
            elif verdict == "inconnu":
                state["notLoggedIn"] = True
                state["errors"].append(product)
            else:
                state["errors"].append(product)
    print(json.dumps(state))


main()
