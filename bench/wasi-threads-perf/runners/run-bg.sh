#!/bin/bash
# start a benchmark run in the background; caller polls the log file.
# usage: run-bg.sh <logfile> <cmd...>
log="$1"; shift
: > "$log"
nohup "$@" >> "$log" 2>&1 &
echo "pid=$!" >> "$log"
