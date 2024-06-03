---
icon: material/microsoft-windows
title: Windows
---

# Nextest on Windows

While nextest generally functions well on Windows, it is usually slower than on Unix platforms. Below are some tips on how to make it go faster.

## Dev Drive

Windows 11 and above have a feature called [Dev Drive](https://learn.microsoft.com/en-us/windows/dev-drive/) that is tuned for development workflows. Moving the following files over to the ReFS dev drive should result in a notable performance improvement:

- Your `CARGO_HOME` or `.cargo\bin` directory, typically within your home directory (see [this Rust issue](https://github.com/rust-lang/cargo/issues/5028)).
- Your source repository
- Cargo's target directory

Depending on your risk tolerance, it may also be worth trying to disable antivirus filters on the dev drive. See [_Understanding security risks and trust in relation to Dev Drive_](https://learn.microsoft.com/en-us/windows/dev-drive/#understanding-security-risks-and-trust-in-relation-to-dev-drive) on MSDN for more information.

## Antivirus

Your antivirus software—typically Windows Security, also known as Microsoft Defender—might interfere with process execution, making your test runs significantly slower. If the Dev Drive feature is not available to you, consider excluding the following directories from checks manually:

- Your `CARGO_HOME` or `.cargo\bin` directory, typically within your home directory (see [this Rust issue](https://github.com/rust-lang/cargo/issues/5028)).
- Your source repository
- Cargo's target directory

[Here's how to exclude directories from Windows Security.](https://support.microsoft.com/en-us/windows/add-an-exclusion-to-windows-security-811816c0-4dfd-af4a-47e4-c301afe13b26)

![Windows Security exclusion list example](../../static/windows-exclusions.png)

!!! info "More information"

    Even with real-time antivirus monitoring disabled, it has been a long-standing fact that **process creation is significantly slower on Windows** than it is on Unix platforms. Because nextest creates a process for every test, this can result in a significant performance penalty on Windows.

    As always, we recommend that you benchmark nextest for your workflows. Nextest's other advantages, such as parallel test execution and the ability to archive and reuse builds, may still make it a net win for your project.

    Suggests and improvements from Windows experts, both to this page and to nextest itself, are hugely welcome.
