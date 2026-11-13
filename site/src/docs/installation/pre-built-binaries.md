---
icon: material/download
description: Installing pre-built nextest binaries with cargo-binstall or platform-specific commands.
---

# Installing pre-built binaries

The quickest way to get going with nextest is to download a pre-built binary for your platform.

## Installing via the install scripts

The easiest way to install nextest is to use the install script. We provide `sh` and PowerShell install scripts:

=== ":material-console: Linux, macOS, WSL"

    Run the following command in your shell. This also works on FreeBSD and illumos, and in Cygwin and Git Bash (MSYS) on Windows.

    ```
    curl -LsSf https://get.nexte.st/install.sh | sh -s --
    ```

    This will install nextest to `$CARGO_HOME/bin`, or `~/.cargo/bin` if `CARGO_HOME` isn't set.

    To pass in options, add them after `--`. For example, to install the latest release in the 0.9 series:

    ```
    curl -LsSf https://get.nexte.st/install.sh | sh -s -- --version 0.9
    ```

=== ":material-microsoft-windows: Windows (PowerShell)"

    Run the following command in your shell (both Windows PowerShell 5.1 and PowerShell 7 are supported):

    ```powershell
    irm https://get.nexte.st/install.ps1 | iex
    ```

    This will install nextest to `$env:CARGO_HOME\bin`, or `~\.cargo\bin` if `CARGO_HOME` isn't set.

    To pass in options, run the script as a script block. For example, to install the latest release in the 0.9 series:

    ```powershell
    & ([scriptblock]::Create((irm https://get.nexte.st/install.ps1))) -Version 0.9
    ```

### Install script options

| Unix shell | PowerShell | Environment variable | Description |
| --- | --- | --- | --- |
| `--version <VERSION>` | `-Version <VERSION>` | `NEXTEST_INSTALL_VERSION` | The version to install: `latest` (the default), a version series like `0.9`, or an exact version like `0.9.150`. |
| `--install-dir <DIR>` | `-InstallDir <DIR>` | `NEXTEST_INSTALL_DIR` | The directory to install nextest into. |
| `--target <TRIPLE>` | `-Target <TRIPLE>` | `NEXTEST_INSTALL_TARGET` | The [target triple](https://doc.rust-lang.org/rustc/platform-support.html) to install, if you'd like a different build from the one that matches your system. |

Options take precedence over environment variables. With `install.sh`, `--help` prints a summary of the options.

`latest` and version series like `0.9` only select stable releases. To stay on the 0.9 series and avoid breaking changes, use `0.9` (see the [stability policy](../stability/index.md) for more). To install a pre-release, pass its exact version, for example `0.9.152-b.1`.

??? info "How the install scripts work"

    - **Choosing a build.** Without `--target`, the build that matches your platform is installed. On Linux, if glibc is missing, or older than the standard build requires, the statically linked [musl](https://musl.libc.org/) build is installed instead. On Windows, the build for your machine's native architecture is installed.
    - **Verification.** The download's SHA-256 checksum is verified before anything is installed.
    - **Existing installs.** An existing `cargo-nextest` binary in the install directory is replaced.
    - **Your `PATH`.** The install scripts don't modify your `PATH`. If the install directory isn't on your `PATH`, you'll see a message saying so.
    
    The Unix install script requires `curl`, `tar`, and `gunzip`, along with one of `sha256sum`, `shasum`, `sha256`, `digest`, or `openssl` for checksum verification.

To update nextest after installing it, run `cargo nextest self update`. For more, see [_Updating nextest_](updating.md).

## Downloading and installing from the release URLs

If you would like to avoid using the install script, nextest also provides stable [release URLs](release-urls.md) for its binaries. The instructions below are suitable for both end users and CI.

=== ":material-language-rust: With cargo-binstall"

    If you have [cargo-binstall](https://github.com/ryankurte/cargo-binstall) available:

    ```
    cargo binstall cargo-nextest --secure
    ```

=== ":material-linux: Linux x86_64"

    !!! info

        The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in a terminal:

    ```
    curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

    For a statically-linked binary with no runtime library dependencies, based on [musl](https://musl.libc.org/):

    ```
    curl -LsSf https://get.nexte.st/latest/linux-musl | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

=== ":material-linux: Linux aarch64"

    !!! info

        The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

        If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

    Run in a terminal:

    ```
    curl -LsSf https://get.nexte.st/latest/linux-arm | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
    ```

    For a statically-linked binary with no runtime library dependencies, based on [musl](https://musl.libc.org/):

    ```
    curl -LsSf https://get.nexte.st/latest/linux-arm-musl | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
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

    === "Using WinGet"

        Run in a terminal:

        ```
        winget install nextest.cargo-nextest
        ```

    === "PowerShell"

        !!! info

            The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

            If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

        Run in PowerShell:

        ```powershell
        $tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
        Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows
        $outputDir = if ($Env:CARGO_HOME) { Join-Path $Env:CARGO_HOME "bin" } else { "~/.cargo/bin" }
        $tmp | Expand-Archive -DestinationPath $outputDir -Force
        $tmp | Remove-Item
        ```

    === "Unix shell"

        !!! info

            The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

            If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

        Using a Unix shell, `curl`, and `tar` _natively_ on Windows (e.g. `shell: bash` on GitHub Actions, or Git Bash):

        ```
        curl -LsSf https://get.nexte.st/latest/windows-tar | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

    Windows Subsystem for Linux (WSL) users should follow the **Linux x86_64** instructions.

??? info "Other platforms"

    === ":material-microsoft-windows: Windows aarch64"

        === "Windows aarch64 using WinGet"

            Run in a terminal:

            ```
            winget install nextest.cargo-nextest
            ```

        === "Windows aarch64 with PowerShell"

            !!! info

                The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

                If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

            Run in PowerShell:

            ```powershell
            $tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
            Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows-arm
            $outputDir = if ($Env:CARGO_HOME) { Join-Path $Env:CARGO_HOME "bin" } else { "~/.cargo/bin" }
            $tmp | Expand-Archive -DestinationPath $outputDir -Force
            $tmp | Remove-Item
            ```

        === "Windows aarch64 with a Unix shell"

            !!! info

                The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

                If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

            Using a Unix shell, `curl`, and `tar` _natively_ on Windows (e.g. `shell: bash` on GitHub Actions, or Git Bash):

            ```
            curl -LsSf https://get.nexte.st/latest/windows-arm-tar | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
            ```

    === ":material-microsoft-windows: Windows i686"

        === "Windows i686 using WinGet"

            Run in a terminal:

            ```
            winget install nextest.cargo-nextest
            ```

        === "Windows i686 with PowerShell"

            !!! info

                The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

                If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

            Run in PowerShell:

            ```powershell
            $tmp = New-TemporaryFile | Rename-Item -NewName { $_ -replace 'tmp$', 'zip' } -PassThru
            Invoke-WebRequest -OutFile $tmp https://get.nexte.st/latest/windows-x86
            $outputDir = if ($Env:CARGO_HOME) { Join-Path $Env:CARGO_HOME "bin" } else { "~/.cargo/bin" }
            $tmp | Expand-Archive -DestinationPath $outputDir -Force
            $tmp | Remove-Item
            ```

        === "Windows i686 with a Unix shell"

            !!! info

                The commands below assume that your Rust installation is managed via [rustup](https://rustup.rs). You can extract the archive to a different directory in your PATH if required.

                If you'd like to stay on the 0.9 series to avoid breaking changes (see the [stability policy](../stability/index.md) for more), replace `latest` in the URL with `0.9`.

            Using a Unix shell, `curl`, and `tar` _natively_ on Windows (e.g. `shell: bash` on GitHub Actions, or Git Bash):

            ```
            curl -LsSf https://get.nexte.st/latest/windows-x86-tar | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
            ```

    === ":material-linux: Linux riscv64"

        Run in a terminal:

        ```
        curl -LsSf https://get.nexte.st/latest/linux-riscv64 | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

    === ":material-freebsd: FreeBSD x86_64"

        ```
        curl -LsSf https://get.nexte.st/latest/freebsd | tar zxf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

    === ":material-bird: illumos x86_64"

        ```
        curl -LsSf https://get.nexte.st/latest/illumos | gunzip | tar xf - -C ${CARGO_HOME:-~/.cargo}/bin
        ```

        As of 2025-04, the current version of illumos tar has [a bug](https://www.illumos.org/issues/15228) where `tar zxf` doesn't work over standard input.

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

    For a full list of environment variables supported by nextest, see [*Environment variables*](../configuration/env-vars.md).

### Other CI systems

Install pre-built binaries on other CI systems by downloading and extracting the respective archives, using the commands above as a guide. See [*Release URLs*](release-urls.md) for more about how to specify nextest versions and platforms.

!!! question "Documentation"

    If you've made it easy to install nextest on another CI system, please feel free to [submit a pull request] with documentation.

[submit a pull request]: https://github.com/nextest-rs/nextest/pulls

## Release URLs

The latest nextest release is available at:

- [**get.nexte.st/latest/linux**](https://get.nexte.st/latest/linux) for Linux x86_64, including Windows Subsystem for Linux (WSL)[^glibc]
- [**get.nexte.st/latest/linux-musl**](https://get.nexte.st/latest/linux-musl) for a statically linked Linux x86_64 binary, including Windows Subsystem for Linux (WSL)
- [**get.nexte.st/latest/linux-arm**](https://get.nexte.st/latest/linux-arm) for Linux aarch64[^glibc]
- [**get.nexte.st/latest/linux-arm-musl**](https://get.nexte.st/latest/linux-arm-musl) for a statically linked Linux aarch64 binary
- [**get.nexte.st/latest/mac**](https://get.nexte.st/latest/mac) for macOS, both x86_64 and Apple Silicon
- [**get.nexte.st/latest/windows**](https://get.nexte.st/latest/windows) for Windows x86_64
- [**get.nexte.st/latest/illumos**](https://get.nexte.st/latest/illumos) for illumos x86_64

??? info "Other platforms"

    Nextest's CI isn't run on these platforms -- these binaries most likely work but aren't guaranteed to do so.

    - [**get.nexte.st/latest/linux-riscv64**](https://get.nexte.st/latest/linux-riscv64) for Linux riscv64gc
    - [**get.nexte.st/latest/windows-x86**](https://get.nexte.st/latest/windows-x86) for Windows i686
    - [**get.nexte.st/latest/freebsd**](https://get.nexte.st/latest/freebsd) for FreeBSD x86_64

These archives contain a single binary called `cargo-nextest` (`cargo-nextest.exe` on Windows). Add this binary to a location on your PATH.

[^glibc]: The standard Linux binaries target glibc, and have a minimum requirement of glibc 2.27 (Ubuntu 18.04).

For a full specification of release URLs, see [_Release URLs_](release-urls.md).

## Code signing policy

Nextest's Windows binaries are digitally signed. Free code signing is provided by SignPath.io, and the certificate by SignPath Foundation.

* Committers and reviewers: [Members team](https://github.com/orgs/nextest-rs/teams/members)
* Approvers: [Owners](https://github.com/orgs/nextest-rs/people?query=role%3Aowner)

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it, e.g. via the [self-update feature](updating.md).
