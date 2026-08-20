import sys
from pathlib import Path

# repin_gates.py sits one level up; it is a script, not an installed package.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
