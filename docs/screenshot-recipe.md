# Recreating the README screenshots

Steps to reproduce the theme-responsive README screenshots with `playwright-cli` against the `explore` example.

## Boot the example

```shell
cargo run --example explore -- mixed-forest --port 13380
```

## Capture the four views

The capture commands use the same scenario in desktop and mobile sizes, then force the stored theme to light and dark. Each pass opens the target URL before resizing because `playwright-cli open` creates a fresh page with the default viewport. Reload after setting the theme so the page paints in the requested mode, then clear highlights, focus, text selection, and scroll position before taking the screenshot. The four `.scratch/screenshots/` files are local working screenshots. Copy them into `docs/screenshots/` after capture so the recipe has checked-in source fixtures for future composition work.

```shell
mkdir -p .scratch/screenshots

# desktop, light
playwright-cli open http://127.0.0.1:13380/?view=all
playwright-cli resize 1400 1000
playwright-cli localstorage-set theme light
playwright-cli reload
playwright-cli eval "() => document.body.style.zoom = '1.6'"
playwright-cli eval "() => document.querySelector('[aria-label=\"Dismiss introduction\"]')?.click()"
playwright-cli eval "() => document.querySelectorAll('main details').forEach(d => d.open = true)"
playwright-cli highlight --hide
playwright-cli press Escape
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.getSelection()?.removeAllRanges()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli screenshot --filename=.scratch/screenshots/desktop-light.png

# desktop, dark
playwright-cli open http://127.0.0.1:13380/?view=all
playwright-cli resize 1400 1000
playwright-cli localstorage-set theme dark
playwright-cli reload
playwright-cli eval "() => document.body.style.zoom = '1.6'"
playwright-cli eval "() => document.querySelector('[aria-label=\"Dismiss introduction\"]')?.click()"
playwright-cli eval "() => document.querySelectorAll('main details').forEach(d => d.open = true)"
playwright-cli highlight --hide
playwright-cli press Escape
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.getSelection()?.removeAllRanges()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli screenshot --filename=.scratch/screenshots/desktop-dark.png

# mobile, light
playwright-cli open http://127.0.0.1:13380/?view=all
playwright-cli resize 390 844
playwright-cli localstorage-set theme light
playwright-cli reload
playwright-cli eval "() => document.body.style.zoom = '1'"
playwright-cli eval "() => document.querySelector('[aria-label=\"Dismiss introduction\"]')?.click()"
playwright-cli eval "() => document.querySelectorAll('main details').forEach(d => d.open = true)"
playwright-cli highlight --hide
playwright-cli press Escape
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.getSelection()?.removeAllRanges()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli screenshot --filename=.scratch/screenshots/mobile-light.png

# mobile, dark
playwright-cli open http://127.0.0.1:13380/?view=all
playwright-cli resize 390 844
playwright-cli localstorage-set theme dark
playwright-cli reload
playwright-cli eval "() => document.body.style.zoom = '1'"
playwright-cli eval "() => document.querySelector('[aria-label=\"Dismiss introduction\"]')?.click()"
playwright-cli eval "() => document.querySelectorAll('main details').forEach(d => d.open = true)"
playwright-cli highlight --hide
playwright-cli press Escape
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.getSelection()?.removeAllRanges()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli screenshot --filename=.scratch/screenshots/mobile-dark.png
```

Copy the source fixtures into the checked-in docs assets directory:

```shell
cp .scratch/screenshots/desktop-light.png docs/screenshots/screenshot-desktop-light.png
cp .scratch/screenshots/desktop-dark.png docs/screenshots/screenshot-desktop-dark.png
cp .scratch/screenshots/mobile-light.png docs/screenshots/screenshot-mobile-light.png
cp .scratch/screenshots/mobile-dark.png docs/screenshots/screenshot-mobile-dark.png
```

## Compose the README images

Build one staging page that places desktop and mobile screenshots in side-by-side front/back device stacks. The page reads from the checked-in `docs/screenshots/screenshot-*.png` source fixtures, then Playwright captures transparent images directly so rounded corners keep their native antialiasing. `docs/screenshots/readme-preview-light.png` puts light screenshots in front and dark screenshots behind. `docs/screenshots/readme-preview-dark.png` swaps only those image sources so dark screenshots sit in front.

```shell
cat > .scratch/screenshots/contact-sheet.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>missing-ebooks screenshots</title>
<svg width="2560" height="1320" viewBox="0 0 2560 1320" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="14" stdDeviation="18" flood-opacity="0.16"/>
    </filter>
    <clipPath id="desktop-clip">
      <rect width="1456" height="1040" rx="58"/>
    </clipPath>
    <clipPath id="mobile-clip">
      <rect width="420" height="908" rx="34"/>
    </clipPath>
  </defs>
  <g class="desktop back" transform="translate(282 204)" filter="url(#shadow)" aria-label="Desktop screenshot behind the foreground desktop screenshot">
    <rect width="1456" height="1040" rx="58" fill="#101827"/>
    <image id="desktop-back" href="../../docs/screenshots/screenshot-desktop-dark.png" width="1456" height="1040" preserveAspectRatio="xMidYMid slice" clip-path="url(#desktop-clip)"/>
  </g>
  <g class="desktop front" transform="translate(62 48)" filter="url(#shadow)" aria-label="Desktop screenshot in front of the background desktop screenshot">
    <rect width="1456" height="1040" rx="58" fill="#101827"/>
    <image id="desktop-front" href="../../docs/screenshots/screenshot-desktop-light.png" width="1456" height="1040" preserveAspectRatio="xMidYMid slice" clip-path="url(#desktop-clip)"/>
  </g>
  <g class="mobile back" transform="translate(2078 264)" filter="url(#shadow)" aria-label="Mobile screenshot behind the foreground mobile screenshot">
    <rect width="420" height="908" rx="34" fill="#101827"/>
    <image id="mobile-back" href="../../docs/screenshots/screenshot-mobile-dark.png" width="420" height="908" preserveAspectRatio="xMidYMid slice" clip-path="url(#mobile-clip)"/>
  </g>
  <g class="mobile front" transform="translate(1918 160)" filter="url(#shadow)" aria-label="Mobile screenshot in front of the background mobile screenshot">
    <rect width="420" height="908" rx="34" fill="#101827"/>
    <image id="mobile-front" href="../../docs/screenshots/screenshot-mobile-light.png" width="420" height="908" preserveAspectRatio="xMidYMid slice" clip-path="url(#mobile-clip)"/>
  </g>
</svg>
HTML

python3 -m http.server 13381 --directory . &
playwright-cli open http://127.0.0.1:13381/.scratch/screenshots/contact-sheet.html
playwright-cli resize 2560 1320
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.getSelection()?.removeAllRanges()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli eval "() => Math.min(...[...document.querySelectorAll('.mobile')].map(el => el.getBoundingClientRect().left)) - Math.max(...[...document.querySelectorAll('.desktop')].map(el => el.getBoundingClientRect().right))"
playwright-cli run-code "async page => await page.screenshot({ path: 'docs/screenshots/readme-preview-light.png', type: 'png', omitBackground: true })"
playwright-cli eval "() => document.getElementById('desktop-front').setAttribute('href', '../../docs/screenshots/screenshot-desktop-dark.png')"
playwright-cli eval "() => document.getElementById('desktop-back').setAttribute('href', '../../docs/screenshots/screenshot-desktop-light.png')"
playwright-cli eval "() => document.getElementById('mobile-front').setAttribute('href', '../../docs/screenshots/screenshot-mobile-dark.png')"
playwright-cli eval "() => document.getElementById('mobile-back').setAttribute('href', '../../docs/screenshots/screenshot-mobile-light.png')"
playwright-cli run-code "async page => await page.screenshot({ path: 'docs/screenshots/readme-preview-dark.png', type: 'png', omitBackground: true })"
```

## Wire the README image

GitHub supports theme-specific images through `<picture>` and `prefers-color-scheme` media queries. Keep the fallback `img` on the light screenshot so Markdown renderers that ignore `<picture>` still show an image.

```html
<a href="https://demo-missing-ebooks.noahbaculi.com">
  <picture>
    <source
      media="(prefers-color-scheme: dark)"
      srcset="docs/screenshots/readme-preview-dark.png"
    />
    <source
      media="(prefers-color-scheme: light)"
      srcset="docs/screenshots/readme-preview-light.png"
    />
    <img
      src="docs/screenshots/readme-preview-light.png"
      alt="missing-ebooks desktop and mobile tree views shown as light and dark front/back stacks"
    />
  </picture>
</a>
```
