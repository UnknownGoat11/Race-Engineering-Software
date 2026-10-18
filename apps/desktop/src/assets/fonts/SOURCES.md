# Bundled Fonts — Sources and Provenance

This directory contains offline bundled static WOFF2 font files for the LMU Race Engineering UI, compliant with Owner Rule OR-6 (no remote font loading, zero network leakage).

## 1. Inter

- **Family**: Inter
- **Author**: Rasmus Andersson
- **License**: SIL Open Font License 1.1 (`OFL-Inter.txt`)
- **Version**: 4.1 (packaged via `@fontsource/inter` 5.3.0)
- **Upstream Repository**: https://github.com/rsms/inter
- **Files & SHA-256 Hashes**:
  - `Inter-Regular.woff2` (Weight 400, Latin): `8909904ab6c872eb994093482a88a28eca2cd95912d7b6fecd72103b0dc07edc`
  - `Inter-Medium.woff2` (Weight 500, Latin): `f3779f1efccc4bdcdf9c0a02ab95bf6bd092ed09c48c08cedc725889edd1d19f`
  - `Inter-SemiBold.woff2` (Weight 600, Latin): `f9a06e79cd3a2a20951c0f0e28f66dd0e6d3fda73911d640a2125c8fcb78f21a`
  - `Inter-Bold.woff2` (Weight 700, Latin): `6f56409fd3d64bb85f7d070bce20749db2d66b6d63cec586cc22d1c761be2491`

## 2. JetBrains Mono

- **Family**: JetBrains Mono
- **Author**: JetBrains / Philipp Nurullin
- **License**: SIL Open Font License 1.1 (`OFL-JetBrainsMono.txt`)
- **Version**: 2.304 (packaged via `@fontsource/jetbrains-mono` 5.3.0)
- **Upstream Repository**: https://github.com/JetBrains/JetBrainsMono
- **Files & SHA-256 Hashes**:
  - `JetBrainsMono-Regular.woff2` (Weight 400, Latin): `14425ba9c695763c1547f48a206b7aa60350a33ae23de09f0407877f3fcd89eb`
  - `JetBrainsMono-SemiBold.woff2` (Weight 600, Latin): `400c6bfda18d5d14acad1c15d6dcb9f8e13c015e7286317e0b9a482539bef147`
  - `JetBrainsMono-Bold.woff2` (Weight 700, Latin): `d0d4e818808f2a0ba39b2b09d1989366f63494e295f003c7ef436697378507e8`

## Verification
- No remote `@import` or `@font-face` URL allowed.
- Fonts are bundled into the binary application bundle by Vite at build time.
