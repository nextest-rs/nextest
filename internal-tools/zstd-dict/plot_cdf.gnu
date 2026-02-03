# Color definitions
set style line 1 lc rgb "#F28E2B" lw 2  # Dark Orange
set style line 2 lc rgb "#59A14F" lw 2  # Green
set style line 3 lc rgb "#4E79A7" lw 2  # Steel Blue

# Plot settings
set terminal pngcairo enhanced font "Arial,13" size 1400,800
set output 'compression_cdf.png'
set title "Test output compression size CDF" font ",18"
set xlabel "size (bytes)"
set ylabel "cumulative fraction"
set key outside right top vertical box
set grid ytics
set yrange [0:1]
set logscale x
set format x "10^{%T}"

# Column 1 is the category label; columns 2-4 are the sizes.
plot 'compression_data.txt' using 2:(1.0) smooth cnormal title "uncompressed" ls 1, \
     '' using 4:(1.0) smooth cnormal title "plain zstd-3" ls 2, \
     '' using 3:(1.0) smooth cnormal title "dict zstd-3" ls 3
