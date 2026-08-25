#!/usr/bin/env python3
import argparse
from pathlib import Path


def read_varint(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 70, 7):
        if offset >= len(data):
            raise ValueError("truncated protobuf varint")
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        if byte < 0x80:
            return value, offset
    raise ValueError("invalid protobuf varint")


def field_end(data: bytes, offset: int, wire_type: int) -> int:
    if wire_type == 0:
        return read_varint(data, offset)[1]
    if wire_type == 1:
        return offset + 8
    if wire_type == 2:
        length, offset = read_varint(data, offset)
        return offset + length
    if wire_type == 5:
        return offset + 4
    raise ValueError(f"unsupported protobuf wire type {wire_type}")


def entry_name(entry: bytes) -> str:
    offset = 0
    while offset < len(entry):
        key, offset = read_varint(entry, offset)
        number, wire_type = key >> 3, key & 7
        if number == 1 and wire_type == 2:
            length, offset = read_varint(entry, offset)
            return entry[offset : offset + length].decode("ascii").lower()
        offset = field_end(entry, offset, wire_type)
        if offset > len(entry):
            raise ValueError("truncated protobuf field")
    raise ValueError("geodata entry has no name")


def filter_data(data: bytes, names: set[str]) -> bytes:
    output = bytearray()
    found = set()
    offset = 0
    while offset < len(data):
        start = offset
        key, offset = read_varint(data, offset)
        if key != 10:
            raise ValueError("unexpected geodata root field")
        length, offset = read_varint(data, offset)
        end = offset + length
        if end > len(data):
            raise ValueError("truncated geodata entry")
        name = entry_name(data[offset:end])
        if name in names:
            output.extend(data[start:end])
            found.add(name)
        offset = end
    missing = names - found
    if missing:
        raise ValueError(f"missing geodata entries: {', '.join(sorted(missing))}")
    return bytes(output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("names", nargs="+")
    args = parser.parse_args()
    names = {name.lower() for name in args.names}
    args.destination.write_bytes(filter_data(args.source.read_bytes(), names))


if __name__ == "__main__":
    main()
