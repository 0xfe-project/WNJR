CC = cc
CFLAGS = -std=c11 -Wall -Wextra -Wno-switch -Wno-missing-field-initializers -Wno-sign-compare -O2

SRCS = main.c tokenize.c preprocess.c unicode.c strings.c hashmap.c type.c parse.c align.c dump.c \
       njvm_mp.c njvm_ir.c codegen.c
OBJS = $(SRCS:.c=.o)

wcc: $(OBJS)
	$(CC) -o $@ $^

$(OBJS): chibicc.h njvm.h

test: wcc
	cd $(CURDIR) && ./test/test.sh
	cd $(CURDIR) && ./test/dump-test.sh

clean:
	rm -f $(OBJS) wcc

.PHONY: test clean
