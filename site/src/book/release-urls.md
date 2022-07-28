# Release URLs

Binary releases of cargo-nextest will always be available at **`https://get.nexte.st/{version}/{platform}`**.

## `{version}` identifier

The `{version}` identifier is:
* `latest` for the latest release (not including pre-releases)
* a version range, for example `0.9`, for the latest release in the 0.9 series (not including pre-releases)
* the exact version number, for example `0.9.4`, for that specific version

## `{platform}` identifier

The `{platform}` identifier is:
* `x86_64-unknown-linux-gnu.tar.gz` for x86_64 Linux (tar.gz)
* `x86_64-unknown-linux-musl.tar.gz` for x86_64 Linux with [musl](https://musl.libc.org/) (tar.gz, available for version 0.9.29+)
* `aarch64-unknown-linux-gnu.tar.gz` for aarch64 Linux (tar.gz, available for version 0.9.29+)
* `universal-apple-darwin.tar.gz` for x86_64 and arm64 macOS (tar.gz)
* `x86_64-pc-windows-msvc.zip` for x86_64 Windows (zip)
* `x86_64-pc-windows-msvc.tar.gz` for x86_64 Windows (tar.gz)
* `i686-pc-windows-msvc.zip` for i686 Windows (zip)
* `i686-pc-windows-msvc.tar.gz` for i686 Windows (tar.gz)

For convenience, the following shortcuts are defined:

* `linux` points to `x86_64-unknown-linux-gnu.tar.gz`
* `linux-musl` points to `x86_64-unknown-linux-musl.tar.gz`
* `linux-arm` points to `aarch64-unknown-linux-gnu.tar.gz`
* `mac` points to `universal-apple-darwin.tar.gz`
* `windows` points to `x86_64-pc-windows-msvc.zip`
* `windows-tar` points to `x86_64-pc-windows-msvc.tar.gz`
* `windows-x86` points to `i686-pc-windows-msvc.zip`
* `windows-x86-tar` points to `i686-pc-windows-msvc.tar.gz`

Also, each release's canonical GitHub Releases URL is available at **`https://get.nexte.st/{version}/release`**. For example, the latest GitHub release is avaiable at [get.nexte.st/latest/release](https://get.nexte.st/latest/release).

### Examples

The latest nextest release in the 0.9 series for macOS is available as a tar.gz file at [get.nexte.st/0.9/mac](https://get.nexte.st/0.9/mac).

Nextest version 0.9.11 for Windows is available as a zip file at [get.nexte.st/0.9.11/windows](https://get.nexte.st/0.9.11/windows), and as a tar.gz file at [get.nexte.st/0.9.11/windows-tar](https://get.nexte.st/0.9.11/windows-tar).
