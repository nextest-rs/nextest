# Installing pre-built binaries

## Downloading and installing from your terminal

This is the easiest way to get going with cargo-nextest. The instructions below are suitable for
both end users and CI. The links below will stay stable.

> NOTE: The instructions below assume that your Rust installation is managed via rustup. You can extract the tarball to a different directory in your PATH if required.

### Linux x86_64

To install the latest release version of cargo-nextest on a Linux x86_64 computer, from [**get.nexte.st/latest/linux**](https://get.nexte.st/latest/linux):

```
curl -LsSF https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
```

### macOS (x86_64 and Apple Silicon)

To install the latest release version of cargo-nextest on a Mac, from [**get.nexte.st/latest/mac**](https://get.nexte.st/latest/mac):

```
curl -LsSF https://get.nexte.st/latest/mac | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
```

This will download a universal binary that works on both Intel and Apple Silicon Macs.

### Windows x86_64

To get the latest release version of cargo-nextest on a Windows x86_64 computer, from [**get.nexte.st/latest/windows**](https://get.nexte.st/latest/windows), run this in PowerShell:

```powershell
$tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows
$outputDir = if ($Env:CARGO_HOME) { $Env:CARGO_HOME } else { "~/.cargo/bin" }
$tmp | Expand-Archive -DestinationPath $outputDir -Force
$tmp | Remove-Item
```

You can also download the zip manually and unzip it to somewhere in your PATH.

> If you're a Windows expert who can come up with a better way to do this, please [file an issue](https://github.com/nextest-rs/nextest/issues/new) with your suggestion!

## Release URLs

Binary releases of cargo-nextest will always be available at **`https://get.nexte.st/{version}/{platform}`**.

### `{version}` identifier

The `{version}` identifier is:
* "latest" for the latest release (not including pre-releases)
* a version range, for example "0.9", for the latest release in the 0.9 series (not including pre-releases)
* the exact version number, for example "0.9.4", for that specific version

### `{platform}` identifier

The `{platform}` identifier is:
* "x86_64-unknown-linux-gnu" for x86_64 Linux (tar.gz)
* "universal-apple-darwin" for x86_64 and aarch64 macOS (tar.gz)
* "x86_64-pc-windows-msvc" for x86_64 Windows (zip)

In addition, the following convenience shortcuts are defined:

* "linux" for "x86_64-unknown-linux-gnu"
* "mac" for "universal-apple-darwin"
* "windows" for "x86_64-pc-windows-msvc"

In addition, each release's canonical GitHub Releases URL is available at **`https://get.nexte.st/{version}/release`**. For example, the latest GitHub release is avaiable at [get.nexte.st/latest/release](https://get.nexte.st/latest/release).

### Examples

The latest release in the 0.9 series for macOS is available as a tar.gz file at [get.nexte.st/0.9/mac](https://get.nexte.st/0.9/mac).

Version 0.9.4 for Windows is available as a zip file at [get.nexte.st/0.9.4/windows](https://get.nexte.st/0.9.4/windows).
