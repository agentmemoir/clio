# The catalog hash

## Purpose

This page defines `catalog_hash` in plain English, so a reader can recompute the value from a published pack without reading the implementation.

The hash is a fingerprint of the tool catalog this build publishes. It appears in the MCP schema pack (`clio mcp schema-export`) and in `clio help --json`, and it exists so that a change to any published tool definition cannot pass unnoticed.

## The algorithm

The algorithm is FNV-1a over 64 bits. The version tag is `fnv1a64`, and the rendered hash always carries that tag as a prefix, so a reader can tell which algorithm produced a value by looking at the value. The published specification of the algorithm is RFC 9923, [The FNV Non-Cryptographic Hash Algorithm](https://www.rfc-editor.org/rfc/rfc9923.html); this page restates the part a recomputation needs and invents nothing.

FNV-1a is not a cryptographic hash. It exists to detect accidental drift, not to resist an adversary who can choose the tools, so never treat a matching `catalog_hash` as evidence of authenticity.

## The input

The hash covers the compact UTF-8 JSON encoding of every tool definition published in the pack's `tools[]` array: each tool's `name`, `description`, `inputSchema` and `outputSchema`, and any other field the pack publishes for that tool. Nothing else is hashed — not the pack's `format`, not its `format_version`, not `mcp_protocol_revision`, and not the definition object described below.

"Compact" means the shortest encoding `serde_json` produces: no spaces, no newlines, and no indentation. Object keys appear in lexicographic order, and array order is preserved exactly as published. Serialize each tool on its own, exactly as the pack publishes it, and use those bytes.

## The order

The bytes are concatenated in catalog registry order: the order the tools appear in `tools[]`. Adjacent encodings follow one another with no separator, no delimiter, and no trailing newline. The tool list is the unit; the hash is taken over the concatenation, never over the individual tools and never over a re-sorted list.

## The rendered form

The rendered form is exactly `fnv1a64:{hash:016x}`: the literal prefix `fnv1a64`, a colon, and the 64-bit result as 16 lowercase hexadecimal digits with leading zeros kept. An example shape is `fnv1a64:0123456789abcdef`.

That rendered form is an external contract. Anything that reads it — a script, a CI check, a downstream pack consumer — may depend on the prefix, the separator, the width, and the lowercase spelling. The form changes only as a deliberate published change, announced in the changelog, and never as a side effect of some other edit.

## Constants

The 64-bit FNV-1a constants are the offset basis `0xcbf29ce484222325` and the prime `0x100000001b3`. The hash starts at the offset basis; then, for each byte of the input in order, XOR the byte into the hash and multiply by the prime modulo 2^64. XOR first, multiply second, once per byte.

## Recomputing the hash

1. Obtain the pack, for example by running `clio mcp schema-export --out pack.json`, or read the same object out of `clio help --json`. The pack's `catalog_hash_definition` restates every rule on this page, so the definition travels with the data it describes.
2. Take the pack's `tools[]` array. Do not reorder it, do not filter it, and do not add a field that the pack does not publish.
3. Serialize each tool, one at a time, as compact JSON with object keys in lexicographic order and arrays in their published order. This is what a canonicalizing serializer such as `serde_json`'s compact writer produces with no pretty-printing.
4. Concatenate the encoded tools in array order with no separator between them, giving one byte string.
5. Hash that byte string with 64-bit FNV-1a: start at `0xcbf29ce484222325`, then for each byte XOR it into the hash and multiply by `0x100000001b3` modulo 2^64.
6. Render the result as `fnv1a64:` followed by the 64-bit value in 16 lowercase hexadecimal digits. The result must equal the pack's `catalog_hash` exactly.

A reader who follows those six steps and lands on a different value has found either a changed tool definition or a broken serialization rule, and both are worth reporting rather than patching the number.

## Where the definition ships

The same definition is published in every pack, under the top-level key `catalog_hash_definition`, and `clio help --json` republishes it beside `catalog_hash` unchanged. It is an object with these fields: `algorithm`, `version`, `hash_bits`, `offset_basis`, `prime`, `input`, `order`, `concatenation`, `rendered_form`, and `rendered_form_is_external_contract`.

The object is machine-readable on purpose: a tool that verifies a pack can read the rules it must apply instead of hard-coding them from this page, and the `rendered_form_is_external_contract` flag says the rendered form may not drift by accident.

## Reference

- RFC 9923, The FNV Non-Cryptographic Hash Algorithm: https://www.rfc-editor.org/rfc/rfc9923.html
