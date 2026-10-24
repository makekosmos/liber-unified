# Liber

Desktop PDF library and reader built on NSH and MuPDF.

## Run on Windows

From the repository root:

```powershell
nsh run --target windows
```

Use **Добавить PDF** to choose a document. The library, reading progress, and appearance preference are stored under `%APPDATA%\Liber`.

## Check

```powershell
cargo build
cargo test
nsh test
```

See [STATUS.md](STATUS.md) for migrated features and remaining work.
