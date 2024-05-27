#!/bin/sh

# A simple sed script to strip ANSI escape codes from a file. Adapted from
# https://stackoverflow.com/a/51141872.

sed 's/\x1B\[[0-9;]\{1,\}[A-Za-z]//g'
