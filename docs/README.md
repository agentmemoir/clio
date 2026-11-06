# Documentation

## Purpose

This directory holds reference documents for people who operate, extend, or audit the MCP surface and the compliance runbook.

## Scope

What belongs here is reader-facing reference: the documents a reader consults to run, extend, or check the product. What does not belong here is design decisions and the reasons behind them; those live with the code they govern.

## Documents

| Document | What it covers |
| --- | --- |
| [catalog-hash.md](catalog-hash.md) | The written definition of `catalog_hash`: the FNV-1a algorithm and its constants, the exact bytes the hash covers, the rendered form and its contract, and a numbered recipe for recomputing it from a published pack. |
| [compliance-operator-runbook.md](compliance-operator-runbook.md) | The full local walk of one subject request by one self-hosted operator, from scope inventory and verification to copy custody, erasure retry, journal backup, the guarded restore drill, and closure. |
| [mcp-session-state-inventory.md](mcp-session-state-inventory.md) | One row per field the MCP protocol session owns, its owner and its readers and writers, classified as domain state or as lifecycle and bookkeeping state. |
| [mcp-tool-result-inventory.md](mcp-tool-result-inventory.md) | One row per bound MCP tool, naming the handler that builds the result, the kind of value it builds, and whether an output schema could describe that value. |
| [philosophy-eval.md](philosophy-eval.md) | Twenty hand-run scenarios that test whether a behaviour follows the beliefs in `PHILOSOPHY.md`, with a scoring rule for a pass and a fail. |
| [tools-list-pagination.md](tools-list-pagination.md) | The `tools/list` page-size extension, the default and clamped page sizes, the opaque cursor and the view digest it carries, and the requests the server refuses. |

## Reading paths

- An operator works through the compliance runbook, and opens the inventory when a tool result needs explaining.
- A maintainer adding an MCP output schema starts at the inventory, then follows the handler cited in the row.
- A reader checking a design against the product philosophy runs the philosophy evaluation, then reads the belief it names.
- A maintainer preparing stateless execution reads the session state inventory, then the candidate's tests.
