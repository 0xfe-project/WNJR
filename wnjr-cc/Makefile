CC = cc
CFLAGS = -std=c11 -Wall -Wextra -Wno-switch -Wno-missing-field-initializers -Wno-sign-compare -O2

SRCS = main.c tokenize.c preprocess.c unicode.c strings.c hashmap.c type.c parse.c align.c dump.c
OBJS = $(SRCS:.c=.o)

wnjr-cc: $(OBJS)
	$(CC) -o $@ $^

$(OBJS): chibicc.h

clean:
	rm -f $(OBJS) wnjr-cc

.PHONY: clean
