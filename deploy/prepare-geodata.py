#!/usr/bin/env python3
"""Prepare the complete V2Ray GeoIP catalog for Gofro's IPv4 dataplane."""

import argparse
import ipaddress
from pathlib import Path


def varint(value: int) -> bytes:
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def read_varint(data: memoryview, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 70, 7):
        if offset == len(data):
            raise ValueError("truncated protobuf integer")
        byte = data[offset]
        offset += 1
        if shift == 63 and byte > 1:
            raise ValueError("protobuf integer overflow")
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
    raise ValueError("protobuf integer overflow")


def fields(data: bytes | memoryview):
    data = memoryview(data)
    offset = 0
    while offset < len(data):
        key, offset = read_varint(data, offset)
        number, wire = key >> 3, key & 7
        if not 1 <= number <= 0x1FFFFFFF:
            raise ValueError("invalid protobuf field number")
        if wire == 0:
            value, offset = read_varint(data, offset)
        elif wire in (1, 2, 5):
            if wire == 2:
                length, offset = read_varint(data, offset)
            else:
                length = 8 if wire == 1 else 4
            end = offset + length
            if end > len(data):
                raise ValueError("truncated protobuf field")
            value = data[offset:end]
            offset = end
        else:
            raise ValueError("unsupported protobuf wire type")
        yield number, wire, value


def message(number: int, data: bytes) -> bytes:
    return varint(number << 3 | 2) + varint(len(data)) + data


def prepare(data: bytes) -> tuple[bytes, int, int]:
    output = bytearray()
    before = after = 0
    for number, wire, entry in fields(data):
        if number != 1:
            continue
        if wire != 2:
            raise ValueError("invalid GeoIP entry")
        tag = b""
        inverse = False
        networks = []
        for number, wire, value in fields(entry):
            if number == 1:
                if wire != 2:
                    raise ValueError("invalid GeoIP tag")
                tag = bytes(value)
                tag.decode("utf-8")
            elif number == 3:
                if wire != 0:
                    raise ValueError("invalid inverse match")
                inverse = value != 0
            elif number == 2:
                if wire != 2:
                    raise ValueError("invalid GeoIP CIDR")
                address = b""
                prefix = 0
                for number, wire, value in fields(value):
                    if number == 1:
                        if wire != 2:
                            raise ValueError("invalid IP address")
                        address = bytes(value)
                    elif number == 2:
                        if wire != 0:
                            raise ValueError("invalid IP prefix")
                        prefix = value
                before += 1
                if len(address) == 4 and prefix <= 32:
                    networks.append(ipaddress.IPv4Network((int.from_bytes(address), prefix), strict=False))
        cooked = bytearray(message(1, tag))
        if inverse:
            cooked.extend(varint(3 << 3) + varint(1))
        else:
            for network in ipaddress.collapse_addresses(networks):
                cidr = message(1, network.network_address.packed)
                if network.prefixlen:
                    cidr += varint(2 << 3) + varint(network.prefixlen)
                cooked.extend(message(2, cidr))
                after += 1
        output.extend(message(1, cooked))
    return bytes(output), before, after


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    original = args.source.read_bytes()
    cooked, before, after = prepare(original)
    temporary = args.destination.with_name(args.destination.name + ".new")
    try:
        temporary.write_bytes(cooked)
        temporary.replace(args.destination)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"GeoIP: {len(original)} -> {len(cooked)} bytes; {before} CIDRs -> {after} merged IPv4 CIDRs")


if __name__ == "__main__":
    main()
