#!/usr/bin/env python3
"""Build dmgez with locally supplied Pomologie HFS+ packages."""

import sys

from compose_filesystem_cli import main


if __name__ == "__main__":
    sys.exit(main())
