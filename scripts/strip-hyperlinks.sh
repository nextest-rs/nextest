#!/bin/sh

# Strip OSC 8 hyperlinks from text while keeping the visible link text.
# OSC 8 format: ESC ] 8 ; params ; URI ST text ESC ] 8 ; ; ST
# where ST (String Terminator) is either BEL (\x07) or ESC \ (\x1B\x5C)

sed -e 's/\x1B]8;;[^\x07]*\x07//g' \
    -e 's/\x1B]8;;[^\x1B]*\x1B\\//g'
