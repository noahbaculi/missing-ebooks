# Recreating the README screenshots

Steps to reproduce the theme-responsive README screenshots with `playwright-cli` against the `explore` example.

## Boot the example

```shell
cargo run --example explore -- mixed-forest --port 13380
```

## Capture the four views

The capture commands use the same scenario in desktop and mobile sizes, then force the stored theme to light and dark. Each pass opens the target URL before resizing because `playwright-cli open` creates a fresh page with the default viewport. Reload after setting the theme so the page paints in the requested mode, then clear highlights, focus, text selection, and scroll position before taking the screenshot. The four `.scratch/screenshots/` files are local working screenshots; copy them into `docs/screenshots/` after capture so the recipe has checked-in source fixtures for future composition work.

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
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
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
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
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
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
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
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
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

Build one staging page that places desktop and mobile screenshots in side-by-side front/back device stacks. The page reads from the checked-in `docs/screenshots/screenshot-*.png` source fixtures, then Playwright captures transparent images directly so rounded corners keep their native antialiasing. `docs/screenshots/readme-preview-light.png` puts light screenshots in front and dark screenshots behind; `docs/screenshots/readme-preview-dark.png` swaps only those image sources so dark screenshots sit in front.

```shell
cat > .scratch/screenshots/contact-sheet.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>missing-ebooks screenshots</title>
<style>
  * { box-sizing: border-box; }
  html,
  body {
    margin: 0;
    width: 2560px;
    height: 1320px;
    overflow: hidden;
    background: transparent;
  }
  body {
    display: grid;
    place-items: center;
  }
  main {
    width: 2300px;
    height: 1160px;
    position: relative;
  }
  .device {
    position: absolute;
    margin: 0;
    overflow: hidden;
    background: #101827;
    box-shadow: 0 14px 36px rgb(0 0 0 / 0.16);
  }
  .desktop {
    width: 1456px;
    height: 1040px;
    border-radius: 58px;
  }
  .mobile {
    width: 420px;
    height: 908px;
    border-radius: 34px;
  }
  .desktop.back {
    left: 152px;
    top: 124px;
  }
  .desktop.front {
    left: -68px;
    top: -32px;
  }
  .mobile.back {
    left: 1948px;
    top: 184px;
  }
  .mobile.front {
    left: 1788px;
    top: 80px;
  }
  .screen {
    width: 100%;
    height: 100%;
    overflow: hidden;
    border-radius: inherit;
  }
  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
<main>
  <figure class="device desktop back" aria-label="Desktop screenshot behind the foreground desktop screenshot">
    <div class="screen"><img id="desktop-back" src="../../docs/screenshots/screenshot-desktop-dark.png" alt=""></div>
  </figure>
  <figure class="device desktop front" aria-label="Desktop screenshot in front of the background desktop screenshot">
    <div class="screen"><img id="desktop-front" src="../../docs/screenshots/screenshot-desktop-light.png" alt=""></div>
  </figure>
  <figure class="device mobile back" aria-label="Mobile screenshot behind the foreground mobile screenshot">
    <div class="screen"><img id="mobile-back" src="../../docs/screenshots/screenshot-mobile-dark.png" alt=""></div>
  </figure>
  <figure class="device mobile front" aria-label="Mobile screenshot in front of the background mobile screenshot">
    <div class="screen"><img id="mobile-front" src="../../docs/screenshots/screenshot-mobile-light.png" alt=""></div>
  </figure>
</main>
HTML

python3 -m http.server 13381 --directory . &
playwright-cli open http://127.0.0.1:13381/.scratch/screenshots/contact-sheet.html
playwright-cli resize 2560 1320
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
playwright-cli eval "() => { const desktop = [...document.querySelectorAll('.desktop')].map(el => el.getBoundingClientRect()); const mobile = [...document.querySelectorAll('.mobile')].map(el => el.getBoundingClientRect()); const desktopRight = Math.max(...desktop.map(box => box.right)); const mobileLeft = Math.min(...mobile.map(box => box.left)); const gutter = mobileLeft - desktopRight; if (gutter < 64) throw new Error('Expected at least 64px between device stacks, got ' + gutter + 'px'); return gutter; }"
playwright-cli run-code "async page => await page.screenshot({ path: 'docs/screenshots/readme-preview-light.png', type: 'png', omitBackground: true })"
playwright-cli eval "() => { document.getElementById('desktop-front').src = '../../docs/screenshots/screenshot-desktop-dark.png'; document.getElementById('desktop-back').src = '../../docs/screenshots/screenshot-desktop-light.png'; document.getElementById('mobile-front').src = '../../docs/screenshots/screenshot-mobile-dark.png'; document.getElementById('mobile-back').src = '../../docs/screenshots/screenshot-mobile-light.png'; }"
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
