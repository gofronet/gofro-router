#!/usr/bin/env python3
import importlib.util
import ipaddress
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).resolve().parents[3] / "deploy/prepare-geodata.py"
spec = importlib.util.spec_from_file_location("prepare_geodata", SCRIPT)
geo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(geo)


def cidr(address, prefix):
    return geo.message(2, geo.message(1, ipaddress.ip_address(address).packed) + bytes([16]) + geo.varint(prefix))


class PrepareGeodata(unittest.TestCase):
    def test_merge_preserves_exact_ipv4_coverage_and_all_tags(self):
        data = geo.message(1, geo.message(1, b"RU")
            + cidr("5.136.0.0", 14) + cidr("5.140.0.0", 14)
            + cidr("5.136.1.0", 24) + cidr("2001:db8::", 32))
        data += geo.message(1, geo.message(1, b"EMPTY") + cidr("::", 0))
        cooked, before, after = geo.prepare(data)
        expected = geo.message(1, geo.message(1, b"RU") + cidr("5.136.0.0", 13))
        expected += geo.message(1, geo.message(1, b"EMPTY"))
        self.assertEqual(cooked, expected)
        self.assertEqual((before, after), (5, 1))

    def test_inverse_and_zero_prefix_have_original_semantics(self):
        inverse = geo.message(1, geo.message(1, b"INVERSE") + bytes([24, 1]) + cidr("1.1.1.1", 32))
        zero = geo.message(1, geo.message(1, b"ALL") + cidr("1.2.3.4", 0))
        cooked, _, _ = geo.prepare(inverse + zero)
        expected = geo.message(1, geo.message(1, b"INVERSE") + bytes([24, 1]))
        expected += geo.message(1, geo.message(1, b"ALL") + geo.message(2, geo.message(1, bytes(4))))
        self.assertEqual(cooked, expected)

    def test_reordered_and_unknown_fields_are_supported(self):
        entry = cidr("8.8.8.8", 32) + geo.message(17, b"ignored") + geo.message(1, b"TEST")
        cooked, _, _ = geo.prepare(geo.message(1, entry))
        self.assertEqual(cooked, geo.message(1, geo.message(1, b"TEST") + cidr("8.8.8.8", 32)))

    def test_truncated_and_invalid_wire_data_fail(self):
        for data in [b"\x0a\x80", b"\x0a\x02\x0a", b"\x0f", b"\x00", bytes([255]) * 10]:
            with self.assertRaises(ValueError):
                geo.prepare(data)


if __name__ == "__main__":
    unittest.main()
