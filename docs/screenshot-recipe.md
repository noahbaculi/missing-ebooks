# Recreating `assets/screenshot.png`

Steps to reproduce the README screenshot with `playwright-cli` against the `explore` example.

## Boot the example

```shell
cargo run --example explore -- mixed-forest --port 13380
playwright-cli open http://127.0.0.1:13380
```

## Frame the page

```shell
playwright-cli resize 1500 1000
playwright-cli click "getByRole('link', { name: 'All folders' })"
playwright-cli eval "() => document.body.style.zoom = '1.6'"
playwright-cli eval "() => document.querySelector('[aria-label=\"Dismiss introduction\"]')?.click()"
```

Expand every `<details>` in main and scroll to the top:

```shell
playwright-cli eval "() => document.querySelectorAll('main details').forEach(d => d.open = true)"
playwright-cli eval "() => window.scrollTo(0, 0)"
```

## Capture

```shell
playwright-cli screenshot --filename=assets/screenshot.png
```
