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

## Using pre-built binaries in CI

Pre-built binaries can be used in continuous integration to speed up test runs.

### Using nextest in GitHub Actions

The easiest way to install nextest in GitHub Actions is to use the [Install Development Tools](https://github.com/marketplace/actions/install-development-tools) action maintained by [Taiki Endo](https://github.com/taiki-e).

To install the latest version of nextest, add this to your job after installing Rust and Cargo:

```yml
- uses: taiki-e/install-action@nextest
```

[See this in practice with nextest's own CI.](https://github.com/nextest-rs/nextest/blob/5b59a5c5d1a051ce651e5d632c93a849f97a9d4b/.github/workflows/ci.yml#L101-L102)

The action will download pre-built binaries from the URL above and add them to `.cargo/bin`.

To install a version series or specific version, use this instead:

```yml
- uses: taiki-e/install-action@v1
- with:
    tool: nextest
    ## version (defaults to "latest") can be a series like 0.9:
    # version: 0.9
    ## version can also be a specific version like 0.9.11:
    # version: 0.9.11
```

> **Tip:** GitHub Actions supports ANSI color codes. To get color support for nextest (and Cargo), add this to your workflow:
>
> ```yml
> env:
>   CARGO_TERM_COLOR: always
> ```
>
> For a full list of environment variables supported by nextest, see [Environment variables](env-vars.md).

### Other CI systems

Install pre-built binaries on other CI systems by downloading and extracting the respective archives, using the commands above as a guide. See [Release URLs](release-urls.md) for more about how to specify nextest versions and platforms.

> If you've made it easy to install nextest on another CI system, feel free to [submit a pull request] with documentation.

[submit a pull request]: https://github.com/nextest-rs/nextest/pulls
