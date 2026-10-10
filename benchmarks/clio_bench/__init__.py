"""clio-bench: a thin, reproducible retrieval-quality harness for Clio.

This package implements the two first benchmark suites (LongMemEval-style and
LoCoMo-style) against the unified system-adapter interface, a one-command
runner, and a calibrated LLM-as-a-judge integration. The remaining three
suites (BEAM, MemoryAgentBench, AMA-Bench) are documented stubs only.

The harness is additive: it drives the public `clio` binary through the
adapter interface and never changes product behavior.
"""

__all__ = ["__version__"]

__version__ = "0.1.0"
