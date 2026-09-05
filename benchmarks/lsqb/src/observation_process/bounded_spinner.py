"""Test-only CPU spinner: expires even if the Rust test coordinator disappears."""

import sys
import time


deadline = time.monotonic() + min(float(sys.argv[1]) if len(sys.argv) > 1 else 5.0, 5.0)
while time.monotonic() < deadline:
    pass
