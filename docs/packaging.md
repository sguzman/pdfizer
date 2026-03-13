# Packaging Notes

`Pdfizer` depends on a dynamically linked Pdfium runtime. Packaging therefore means shipping two things together:

1. the `pdfizer` executable
2. the correct Pdfium shared library for the target platform

## Runtime Lookup

The app resolves Pdfium in this order:

1. `pdfium.library_path` from config
2. `PDFIUM_DYNAMIC_LIB_PATH`
3. system library lookup

For packaged builds, the most predictable approach is to ship Pdfium next to the executable and point `pdfium.library_path` at that bundled location.

## Platform Notes

### Linux

- Bundle `libpdfium.so`
- Set `PDFIUM_DYNAMIC_LIB_PATH` or write an absolute path into the shipped config
- Verify target distro compatibility for glibc and system graphics libraries

### Windows

- Bundle `pdfium.dll`
- Keep the DLL beside the executable or configure `pdfium.library_path`
- Validate the app on a clean machine without a developer toolchain installed

### macOS

- Bundle the Pdfium dynamic library in the app package
- Ensure the packaged library path resolves correctly after notarization/signing steps

## Release Checklist

- Build the executable in release mode
- Bundle the correct Pdfium library for the target platform
- Include a default config with the runtime path strategy you want to support
- Smoke-test opening a real PDF on a machine that does not already have Pdfium installed
- Confirm benchmark and log directories are writable

## Suggested Release Command

```bash
cargo build --release
```

Then package `target/release/pdfizer` together with the Pdfium shared library and the desired config file.
