---
icon: material/download
---

# Installing pre-built binaries

The quickest way to get going with nextest is to download a pre-built binary for your platform.

## Downloading and installing from your terminal

The instructions below are suitable for both end users and CI. These links will stay stable.

=== ":material-language-rust: With cargo-binstall"

    If you have [cargo-binstall](https://github.com/ryankurte/cargo-binstall) available:

    ```
    cargo binstall cargo-nextest --secure
    ```

=== ":material-linux: Linux x86_64"

    !!! info

        The command below assumes that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in a terminal:

    ```
    curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

=== ":material-linux: Linux aarch64"

    !!! info

        The command below assumes that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in a terminal:

    ```
    curl -LsSf https://get.nexte.st/latest/linux-arm | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

=== ":material-apple: macOS universal"

    !!! info

        The command below assumes that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in a terminal:

    ```
    curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

    This will download a universal binary that works on both Intel and Apple Silicon Macs.

=== ":material-microsoft-windows: Windows x86_64"

    !!! info

        The command below assumes that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in PowerShell:

    ```powershell
    $tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
    Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows
    $outputDir = if ($Env:CARGO_HOME) { Join-Path $Env:CARGO_HOME "bin" } else { "~/.cargo/bin" }
    $tmp | Expand-Archive -DestinationPath $outputDir -Force
    $tmp | Remove-Item
    ```

    Or, using a Unix shell, `curl`, and `tar` _natively_ on Windows (e.g. `shell: bash` on GitHub Actions):

    ```
    curl -LsSf https://get.nexte.st/latest/windows-tar | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

    Windows Subsystem for Linux (WSL) users should follow the **Linux x86_64** instructions.

??? info "Other platforms"

    === "FreeBSD x86_64"

        ```
        curl -LsSf https://get.nexte.st/latest/freebsd | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

    === "illumos x86_64"

        ```
        curl -LsSf https://get.nexte.st/latest/illumos | gunzip | tar xf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

        As of 2022-12, the current version of illumos tar has [a bug](https://www.illumos.org/issues/15228) where `tar zxf` doesn't work over standard input.

## Community-maintained binaries

These binaries are maintained by the community—_thank you!_

=== ":simple-homebrew: Homebrew"

    Run in a terminal on macOS or Linux:

    ```
    brew install cargo-nextest
    ```

=== ":material-arch: Arch Linux"

    Run in a terminal:

    ```
    pacman -S cargo-nextest
    ```

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

```yaml
- uses: taiki-e/install-action@v2
  with:
    tool: nextest
    ## version (defaults to "latest") can be a series like 0.9:
    # tool: nextest@0.9
    ## version can also be a specific version like 0.9.11:
    # tool: nextest@0.9.11
```

!!! tip "ANSI color codes"

    GitHub Actions supports ANSI color codes. To get color support for nextest (and Cargo), add this to your workflow:

    ```yaml
    env:
      CARGO_TERM_COLOR: always
    ```

    For a full list of environment variables supported by nextest, see [Environment variables](../configuration/env-vars.md).

### Other CI systems

Install pre-built binaries on other CI systems by downloading and extracting the respective archives, using the commands above as a guide. See [Release URLs](release-urls.md) for more about how to specify nextest versions and platforms.

!!! question "Documentation"

    If you've made it easy to install nextest on another CI system, please feel free to [submit a pull request] with documentation.

[submit a pull request]: https://github.com/nextest-rs/nextest/pulls

## Release URLs

The latest nextest release is available at:

- [**get.nexte.st/latest/linux**](https://get.nexte.st/latest/linux) for Linux x86_64, including Windows Subsystem for Linux (WSL)[^glibc]
- [**get.nexte.st/latest/linux-arm**](https://get.nexte.st/latest/linux-arm) for Linux aarch64[^glibc]
- [**get.nexte.st/latest/mac**](https://get.nexte.st/latest/mac) for macOS, both x86_64 and Apple Silicon
- [**get.nexte.st/latest/windows**](https://get.nexte.st/latest/windows) for Windows x86_64

??? info "Other platforms"

    Nextest's CI isn't run on these platforms -- these binaries most likely work but aren't guaranteed to do so.

    - [**get.nexte.st/latest/linux-musl**](https://get.nexte.st/latest/linux-musl) for Linux x86_64, with musl libc[^musl]
    - [**get.nexte.st/latest/windows-x86**](https://get.nexte.st/latest/windows-x86) for Windows i686
    - [**get.nexte.st/latest/freebsd**](https://get.nexte.st/latest/freebsd) for FreeBSD x86_64
    - [**get.nexte.st/latest/illumos**](https://get.nexte.st/latest/illumos) for illumos x86_64

These archives contain a single binary called `cargo-nextest` (`cargo-nextest.exe` on Windows). Add this binary to a location on your PATH.

[^glibc]: The standard Linux binaries target glibc, and have a minimum requirement of glibc 2.27 (Ubuntu 18.04).
[^musl]: Rust's musl target currently has [a bug](https://github.com/rust-lang/rust/issues/99740) that Rust's glibc target doesn't have. This bug means that nextest's linux-musl binary has slower test runs and is susceptible to signal-related races. Only use the linux-musl binary if the standard Linux binary doesn't work in your environment.

For a full specification of release URLs, see [_Release URLs_](release-urls.md).
