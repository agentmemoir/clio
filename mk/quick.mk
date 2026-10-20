# Quick: the fast local check. It runs the tests of the modules a change touched,
# without instrumentation, and stops at the first failure. CI owns the full test
# and coverage verdict; Quick judges nothing. See scripts/quick.py.
#
#   make quick BASE=origin/master
#   make quick BASE=origin/master PLAN=1       # print the selection and estimate, run nothing
#
# Like the other scoped gates, BASE defaults to the local master branch, which may
# be stale: pass origin/master.
.PHONY: quick scripts-check-quick

quick:
	@python3 scripts/quick.py --base $(or $(BASE),master) $(if $(PLAN),--plan)

scripts-check-quick:
	python3 scripts/quick_selftest.py
	python3 scripts/quick_untested_selftest.py
	python3 scripts/schema_args_selftest.py
	python3 scripts/schema_args_check.py
