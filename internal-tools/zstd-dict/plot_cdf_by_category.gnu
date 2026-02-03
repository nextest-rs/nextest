# Data preprocessing: split by category (stdout vs stderr).
system "awk '$1 == \"stdout\" {print $2, $3, $4}' compression_data.txt > stdout.tmp"
system "awk '$1 == \"stderr\" {print $2, $3, $4}' compression_data.txt > stderr.tmp"

# Color definitions: color encodes the compression method,
# dash type encodes the category (solid = stdout, dashed = stderr).
set style line 1 lc rgb "#F28E2B" lw 2 dt 1        # Dark Orange, solid (stdout uncompressed)
set style line 2 lc rgb "#F28E2B" lw 2 dt (6,3)   # Dark Orange, dashed (stderr uncompressed)
set style line 3 lc rgb "#59A14F" lw 2 dt 1        # Green, solid (stdout plain)
set style line 4 lc rgb "#59A14F" lw 2 dt (6,3)    # Green, dashed (stderr plain)
set style line 5 lc rgb "#4E79A7" lw 2 dt 1        # Steel Blue, solid (stdout dict)
set style line 6 lc rgb "#4E79A7" lw 2 dt (6,3)    # Steel Blue, dashed (stderr dict)

# Plot settings
set terminal pngcairo enhanced font "Arial,13" size 1400,800
set output 'compression_cdf_by_category.png'
set title "Test output compression size CDF by category" font ",18"
set xlabel "size (bytes)"
set ylabel "cumulative fraction"
set key outside right top vertical box
set grid ytics
set yrange [0:1]
set logscale x
set format x "10^{%T}"

# Columns in the tmp files: uncompressed(1) dict_compressed(2) plain_compressed(3).
plot 'stdout.tmp' using 1:(1.0) smooth cnormal title "stdout uncompressed" ls 1, \
     'stderr.tmp' using 1:(1.0) smooth cnormal title "stderr uncompressed" ls 2, \
     'stdout.tmp' using 3:(1.0) smooth cnormal title "stdout plain zstd-3" ls 3, \
     'stderr.tmp' using 3:(1.0) smooth cnormal title "stderr plain zstd-3" ls 4, \
     'stdout.tmp' using 2:(1.0) smooth cnormal title "stdout dict zstd-3" ls 5, \
     'stderr.tmp' using 2:(1.0) smooth cnormal title "stderr dict zstd-3" ls 6

# Clean up temporary files.
system "rm -f stdout.tmp stderr.tmp"
