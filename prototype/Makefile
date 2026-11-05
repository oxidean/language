CC ?= cc
CFLAGS = -O2 -Wall -Wextra

all: task_size check_cost

task_size: task_size.c
	$(CC) $(CFLAGS) -pthread -o $@ $<

check_cost: check_cost.c
	$(CC) $(CFLAGS) -o $@ $<

run: all
	./task_size
	./check_cost

clean:
	rm -f task_size check_cost

.PHONY: all run clean
