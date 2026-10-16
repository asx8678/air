# make bench VARIANT=<git ref|opencode> MODEL=deepseek-v4.1-flash N=5 [TASKS=a,b] [JOBS=4] [LABEL=name]
# make report BASE=<label> VARIANT=<label> [OUT=file.md]
PYTHON ?= python3
MODEL ?= deepseek-v4.1-flash
VARIANT ?= HEAD
N ?= 5
JOBS ?= 4
TASKS ?=
LABEL ?=
BASE ?=
OUT ?=

bench:
	$(PYTHON) bench.py run --variant $(VARIANT) --model $(MODEL) --n $(N) --jobs $(JOBS) $(if $(TASKS),--tasks $(TASKS)) $(if $(LABEL),--label $(LABEL))

report:
	$(PYTHON) bench.py report --baseline $(BASE) --variant $(VARIANT) --model $(MODEL) $(if $(OUT),--out $(OUT))

.PHONY: bench report
