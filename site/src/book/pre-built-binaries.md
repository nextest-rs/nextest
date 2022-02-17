# Installing pre-built binaries

The quickest way to get going with nextest is to download a pre-built binary for your platform. The latest nextest release is available at:
* [**get.nexte.st/latest/linux**](https://get.nexte.st/latest/linux) for Linux x86_64, including Windows Subsystem for Linux (WSL)
* [**get.nexte.st/latest/mac**](https://get.nexte.st/latest/mac) for macOS, both x86_64 and Apple Silicon
* [**get.nexte.st/latest/windows**](https://get.nexte.st/latest/windows) for Windows x86_64

These archives contain a single binary called `cargo-nextest`. Add this binary to a location on your PATH.

## Downloading and installing from your terminal

The instructions below are suitable for both end users and CI. These links will stay stable.

> NOTE: The instructions below assume that your Rust installation is managed via rustup. You can extract the archive to a different directory in your PATH if required.
>
> If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](stability.md) for more), replace `latest` in the URL with `0.9`.

### Linux x86_64

```
curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
```

### macOS (x86_64 and Apple Silicon)

```
curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
```

This will download a universal binary that works on both Intel and Apple Silicon Macs.

### Windows x86_64 using PowerShell

Run this in PowerShell:

```powershell
$tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows
$outputDir = if ($Env:CARGO_HOME) { Join-Path $Env:CARGO_HOME "bin" } else { "~/.cargo/bin" }
$tmp | Expand-Archive -DestinationPath $outputDir -Force
$tmp | Remove-Item
```

### Windows x86_64 using Unix tools

If you have access to a Unix shell, `curl` and `tar` *natively* on Windows (for example if you're using `shell: bash` on GitHub Actions):

```
curl -LsSf https://get.nexte.st/latest/windows-tar | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
```

> NOTE: Windows Subsystem for Linux (WSL) users should follow the [Linux x86_64 instructions](#linux-x86_64).
>
> If you're a Windows expert who can come up with a better way to do this, please [add a suggestion to this issue](https://github.com/nextest-rs/nextest/issues/31)!

### Example: using a matrix strategy for cross-platform CI on GitHub Actions

Here's an example for how to use a matrix strategy to perform cross-platform testing via GitHub Actions.

```yml
jobs:
  build:
    name: Run tests
    runs-on: ${{ matrix.os.runs-on }}
    strategy:
      matrix:
        os:
          - runs-on: ubuntu-latest
            nextest-platform: linux
          - runs-on: macos-latest
            nextest-platform: mac
          - runs-on: windows-latest
            nextest-platform: windows-tar
        rust-version: [ stable, beta ]
    steps:
      #
      # ... check out your code and install Rust here
      #
      - name: Install latest nextest release
        # The following line is required for Windows compatibility
        shell: bash
        run: |
          curl -LsSf https://get.nexte.st/latest/${{ matrix.os.nextest-platform }} | \
            tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
      - name: Test with latest nextest release
        run: |
          cargo nextest run
```

[See this in practice with nextest's own CI.](https://github.com/nextest-rs/nextest/blob/e43ac449f53fd34e58136cd94b7a72add201fe5a/.github/workflows/ci.yml#L104-L107)

## Release URLs

Binary releases of cargo-nextest will always be available at **`https://get.nexte.st/{version}/{platform}`**.

### `{version}` identifier

The `{version}` identifier is:
* `latest` for the latest release (not including pre-releases)
* a version range, for example `0.9`, for the latest release in the 0.9 series (not including pre-releases)
* the exact version number, for example `0.9.4`, for that specific version

### `{platform}` identifier

The `{platform}` identifier is:
* `x86_64-unknown-linux-gnu.tar.gz` for x86_64 Linux (tar.gz)
* `universal-apple-darwin.tar.gz` for x86_64 and arm64 macOS (tar.gz)
* `x86_64-pc-windows-msvc.zip` for x86_64 Windows (zip)
* `x86_64-pc-windows-msvc.tar.gz` for x86_64 Windows (tar.gz)

For convenience, the following shortcuts are defined:

* `linux` points to `x86_64-unknown-linux-gnu.tar.gz`
* `mac` points to `universal-apple-darwin.tar.gz`
* `windows` points to `x86_64-pc-windows-msvc.zip`
* `windows-tar` points to `x86_64-pc-windows-msvc.tar.gz`

Also, each release's canonical GitHub Releases URL is available at **`https://get.nexte.st/{version}/release`**. For example, the latest GitHub release is avaiable at [get.nexte.st/latest/release](https://get.nexte.st/latest/release).

### Examples

The latest release in the 0.9 series for macOS is available as a tar.gz file at [get.nexte.st/0.9/mac](https://get.nexte.st/0.9/mac).

Version 0.9.4 for Windows is available as a zip file at [get.nexte.st/0.9.4/windows](https://get.nexte.st/0.9.4/windows), and as a tar.gz file at [get.nexte.st/0.9.4/windows-tar](https://get.nexte.st/0.9.4/windows-tar).
