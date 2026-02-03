all: compression_cdf.png compression_cdf_by_category.png

compression_cdf.png: compression_data.txt plot_cdf.gnu
	gnuplot plot_cdf.gnu

compression_cdf_by_category.png: compression_data.txt plot_cdf_by_category.gnu
	gnuplot plot_cdf_by_category.gnu
