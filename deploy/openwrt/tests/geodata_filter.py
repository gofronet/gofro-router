#!/usr/bin/env python3
import importlib.util
import sys
from pathlib import Path

sys.dont_write_bytecode = True
SCRIPT = Path(__file__).parents[1] / "filter_geodata.py"
SPEC = importlib.util.spec_from_file_location("filter_geodata", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def varint(value: int) -> bytes:
    output = bytearray()
    while value > 0x7F:
        output.append((value & 0x7F) | 0x80)
        value >>= 7
    output.append(value)
    return bytes(output)


def entry(name: str) -> bytes:
    encoded = name.encode("ascii")
    message = b"\x0a" + varint(len(encoded)) + encoded + b"\x10\x01"
    return b"\x0a" + varint(len(message)) + message


ru = entry("RU")
category_ru = entry("CATEGORY-RU")
source = entry("US") + ru + category_ru
assert MODULE.filter_data(source, {"ru"}) == ru
assert MODULE.filter_data(source, {"ru", "category-ru"}) == ru + category_ru

try:
    MODULE.filter_data(source, {"missing"})
except ValueError as error:
    assert str(error) == "missing geodata entries: missing"
else:
    raise AssertionError("missing entry was accepted")
