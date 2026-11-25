"""Decode column view stores with the independent ds_store 1.3.4 parser."""

from importlib.metadata import version
from pathlib import Path
import sys

from ds_store import DSStore

STORE = ".DS_Store"
ROOT_KEY = "."
VIEW_STYLE = "icvl"
ICON_OPTIONS = "icvp"
KEYED_LIST_OPTIONS = "lsvp"
ORDERED_LIST_OPTIONS = "lsvC"
COLUMN_VIEW = (b"type", b"clmv")
LIST_VIEW = (b"type", b"Nlsv")
DOCUMENTATION = "Documentation"
CHANGELOG = "Changelog"


def present(store, code):
    try:
        store[ROOT_KEY][code]
    except KeyError:
        return False
    return True


def verify_stores(root):
    assert version("ds_store") == "1.3.4"
    with DSStore.open(str(root / STORE), "r") as store:
        assert store[ROOT_KEY][VIEW_STYLE] == COLUMN_VIEW
        assert store[ROOT_KEY]["vSrn"] == (b"long", 1)
        assert present(store, "bwsp")
        for code in (ICON_OPTIONS, KEYED_LIST_OPTIONS, ORDERED_LIST_OPTIONS):
            assert not present(store, code), code
        assert store[DOCUMENTATION]["Iloc"] == (55, 70)

    with DSStore.open(str(root / DOCUMENTATION / STORE), "r") as store:
        assert store[ROOT_KEY][VIEW_STYLE] == COLUMN_VIEW
        assert store[ROOT_KEY][ICON_OPTIONS]["iconSize"] == 96
        assert store[ROOT_KEY][KEYED_LIST_OPTIONS]["textSize"] == 14
        assert store[ROOT_KEY][ORDERED_LIST_OPTIONS][0] == b"blob"

    with DSStore.open(str(root / CHANGELOG / STORE), "r") as store:
        assert store[ROOT_KEY][VIEW_STYLE] == LIST_VIEW
        assert not present(store, ICON_OPTIONS)
        assert present(store, KEYED_LIST_OPTIONS)
        assert present(store, ORDERED_LIST_OPTIONS)


if __name__ == "__main__":
    verify_stores(Path(sys.argv[1]))
