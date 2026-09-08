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

## Compose the project card image

The README preview is 2560x1320, a 1.94:1 frame built from four windows. That shape is wrong for a project card in a grid of taller neighbours, and four windows at card size leaves each one too small to make anything out. `docs/screenshots/card-preview-light.png` and `card-preview-dark.png` are the card-shaped alternative: 2400x1600 (1.5:1), two desktop windows instead of four, same front/back stack and same transparent capture.

The width is exactly 2400 because the consuming site generates 400/800/1600/2400 wide variants and skips any target wider than the source. A source of exactly 2400 gets all four with no rounding.

The screen rect is 1754x1253, a 1.4:1 box matching the 1400x1000 source aspect, so `preserveAspectRatio="xMidYMid slice"` crops nothing. The corner radius and drop shadow are scaled from the README sheet by the same factor as the rect.

Mind the bottom margin. With `omitBackground: true` a drop shadow clipped by the canvas edge leaves transparency on both sides of the cut, so there is no hard line to notice. The back plate's shadow reaches about `dy + 3 * stdDeviation`, or 83px below the rect, and the group sits at y=258 with its bottom edge at 1511. That leaves 89px, which clears. Moving the groups down would not.

```shell
mkdir -p .scratch/screenshots
cat > .scratch/screenshots/card-sheet.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>missing-ebooks project card</title>
<svg width="2400" height="1600" viewBox="0 0 2400 1600" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="17" stdDeviation="22" flood-opacity="0.16"/>
    </filter>
    <clipPath id="desktop-clip">
      <rect width="1754" height="1253" rx="70"/>
    </clipPath>
  </defs>
  <g class="desktop back" transform="translate(455 258)" filter="url(#shadow)" aria-label="Desktop screenshot behind the foreground desktop screenshot">
    <rect width="1754" height="1253" rx="70" fill="#101827"/>
    <image id="desktop-back" href="../../docs/screenshots/screenshot-desktop-dark.png" width="1754" height="1253" preserveAspectRatio="xMidYMid slice" clip-path="url(#desktop-clip)"/>
  </g>
  <g class="desktop front" transform="translate(190 70)" filter="url(#shadow)" aria-label="Desktop screenshot in front of the background desktop screenshot">
    <rect width="1754" height="1253" rx="70" fill="#101827"/>
    <image id="desktop-front" href="../../docs/screenshots/screenshot-desktop-light.png" width="1754" height="1253" preserveAspectRatio="xMidYMid slice" clip-path="url(#desktop-clip)"/>
  </g>
</svg>
HTML

python3 -m http.server 13381 --directory . &
playwright-cli open http://127.0.0.1:13381/.scratch/screenshots/card-sheet.html
playwright-cli resize 2400 1600
playwright-cli eval "() => document.activeElement?.blur()"
playwright-cli eval "() => window.scrollTo(0, 0)"
playwright-cli run-code "async page => await page.screenshot({ path: 'docs/screenshots/card-preview-light.png', type: 'png', omitBackground: true })"
playwright-cli eval "() => document.getElementById('desktop-front').setAttribute('href', '../../docs/screenshots/screenshot-desktop-dark.png')"
playwright-cli eval "() => document.getElementById('desktop-back').setAttribute('href', '../../docs/screenshots/screenshot-desktop-light.png')"
playwright-cli run-code "async page => await page.screenshot({ path: 'docs/screenshots/card-preview-dark.png', type: 'png', omitBackground: true })"
```

Verify the result before committing. `sips` confirms the dimensions and that the alpha channel survived:

```shell
sips -g pixelWidth -g pixelHeight -g hasAlpha docs/screenshots/card-preview-light.png docs/screenshots/card-preview-dark.png
```

Then check the shadows against the canvas edges. The browser is already open and already serving the repo root, so the cheapest check is to draw each PNG into a canvas and read the alpha of the outermost row and column on all four sides. Every value must be 0. Anything higher means the shadow ran into the edge and got cut:

```shell
playwright-cli eval "async () => {
  const out = {};
  for (const name of ['light', 'dark']) {
    const img = new Image();
    img.src = 'http://127.0.0.1:13381/docs/screenshots/card-preview-' + name + '.png';
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.width; c.height = img.height;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0);
    const d = ctx.getImageData(0, 0, c.width, c.height).data;
    const a = (x, y) => d[(y * c.width + x) * 4 + 3];
    let top = 0, bottom = 0, left = 0, right = 0;
    for (let x = 0; x < c.width; x++) { top = Math.max(top, a(x, 0)); bottom = Math.max(bottom, a(x, c.height - 1)); }
    for (let y = 0; y < c.height; y++) { left = Math.max(left, a(0, y)); right = Math.max(right, a(c.width - 1, y)); }
    out[name] = { top, bottom, left, right };
  }
  return out;
}"
```

Last, look at the image at the size it will actually be seen. The card renders around 320px wide, and a composition that reads at 2400px can turn to mush there:

```shell
cp docs/screenshots/card-preview-light.png .scratch/screenshots/card-320.png
sips -Z 320 .scratch/screenshots/card-320.png
```
