"""Serveur Laya persistant : une requete JSON par ligne, une reponse par ligne.

Le modele se charge une fois (plusieurs secondes) puis repond en quelques
dizaines de millisecondes : c'est tout l'interet de le garder vivant.
"""
import json
import os
import sys

# Les bibliotheques ecrivent volontiers sur la sortie standard ; le protocole,
# lui, ne doit y passer que par `emit`.
_sortie = sys.stdout
sys.stdout = sys.stderr


def emit(obj):
    _sortie.write(json.dumps(obj, ensure_ascii=False, default=_serialisable) + "\n")
    _sortie.flush()


def _serialisable(o):
    if hasattr(o, "item"):
        return o.item()
    if hasattr(o, "tolist"):
        return o.tolist()
    return str(o)


def charger():
    import laya

    choix = os.environ.get("LAYA_CHECKPOINT", "multilingual").strip().lower()
    device = os.environ.get("LAYA_DEVICE", "").strip() or None
    if choix == "router":
        from laya import Router

        return Router(preload=True, device=device) if device else Router(preload=True)
    depot = "convaiinnovations/laya"
    sous = {"english": None, "multilingual": "multilingual", "typed-decisions": "typed-decisions"}
    if choix not in sous:
        raise ValueError("checkpoint inconnu : " + choix)
    kwargs = {}
    if sous[choix]:
        kwargs["subfolder"] = sous[choix]
    if device:
        kwargs["device"] = device
    return laya.load(depot, **kwargs)


def main():
    try:
        agent = charger()
    except Exception as e:  # noqa: BLE001 - le message part vers l'hote
        emit({"ready": False, "error": "%s: %s" % (type(e).__name__, e)})
        return 1
    emit({"ready": True})
    for ligne in sys.stdin:
        ligne = ligne.strip()
        if not ligne:
            continue
        rid = None
        try:
            req = json.loads(ligne)
            rid = req.get("id")
            if req.get("op") == "ping":
                emit({"id": rid, "ok": True})
                continue
            res = agent.predict(req["state"], req["questions"])
            emit({"id": rid, "ok": True, "answers": res["answers"]})
        except Exception as e:  # noqa: BLE001
            emit({"id": rid, "ok": False, "error": "%s: %s" % (type(e).__name__, e)})
    return 0


if __name__ == "__main__":
    sys.exit(main())
