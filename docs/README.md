# Documentation

## Purpose

This directory holds reference documents for people who operate, extend, or audit the MCP surface and the compliance runbook.

## Scope

What belongs here is reader-facing reference: the documents a reader consults to run, extend, or check the product. What does not belong here is design decisions and the reasons behind them; those live with the code they govern.

## Documents

| Document | What it covers |
| --- | --- |
| [compliance-operator-runbook.md](compliance-operator-runbook.md) | The full local walk of one subject request by one self-hosted operator, from scope inventory and verification to copy custody, erasure retry, journal backup, the guarded restore drill, and closure. |
| [mcp-tool-result-inventory.md](mcp-tool-result-inventory.md) | One row per bound MCP tool, naming the handler that builds the result, the kind of value it builds, and whether an output schema could describe that value. |
| [philosophy-eval.md](philosophy-eval.md) | Twenty hand-run scenarios that test whether a behaviour follows the beliefs in `PHILOSOPHY.md`, with a scoring rule for a pass and a fail. |

## Reading paths

- An operator works through the compliance runbook, and opens the inventory when a tool result needs explaining.
- A maintainer adding an MCP output schema starts at the inventory, then follows the handler cited in the row.
- A reader checking a design against the product philosophy runs the philosophy evaluation, then reads the belief it names.
