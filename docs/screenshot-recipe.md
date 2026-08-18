# Recreating the README screenshots

Steps to reproduce the theme-responsive README screenshots with `playwright-cli` against the `explore` example.

The composition step needs Pillow. Install it into whichever Python environment you use for local screenshot work:

```shell
python3 -m pip install --user Pillow
```

## Boot the example

```shell
cargo run --example explore -- mixed-forest --port 13380
```

## Capture the four views

The capture commands use the same scenario in desktop and mobile sizes, then force the stored theme to light and dark. Each pass opens the target URL before resizing because `playwright-cli open` creates a fresh page with the default viewport. Reload after setting the theme so the page paints in the requested mode, then clear highlights, focus, text selection, and scroll position before taking the screenshot.

```shell
mkdir -p .scratch/screenshots

# desktop, light
playwright-cli open http://127.0.0.1:13380/?view=all
playwright-cli resize 1500 1000
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
playwright-cli resize 1500 1000
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

## Compose the README images

Build flat diagonal composites first, then place those images inside the final rounded frames. Both final images keep the dominant theme on the left: `assets/screenshot-light.png` is mostly light on the left, and `assets/screenshot-dark.png` is mostly dark on the left. The page is captured once on black and once on white for each final image, then Pillow reconstructs alpha from each pair so only the outside canvas becomes transparent.

```shell
python3 - <<'PY'
import math

from PIL import Image, ImageDraw

ANGLE_DEGREES = 65
RESAMPLE = Image.Resampling.LANCZOS


def cover(path, size):
    im = Image.open(path).convert('RGB')
    scale = max(size[0] / im.width, size[1] / im.height)
    resized = im.resize((round(im.width * scale), round(im.height * scale)), RESAMPLE)
    return resized.crop((0, 0, size[0], size[1]))


def diagonal(left_path, right_path, out_path, size, reveal_ratio):
    left = cover(left_path, size)
    right = cover(right_path, size)
    scale = 4
    hi_size = (size[0] * scale, size[1] * scale)
    mask = Image.new('L', hi_size, 0)
    slope = math.tan(math.radians(ANGLE_DEGREES))
    center_x = hi_size[0] * reveal_ratio
    center_y = hi_size[1] / 2
    top_x = center_x + center_y / slope
    bottom_x = center_x - center_y / slope
    ImageDraw.Draw(mask).polygon([(0, 0), (top_x, 0), (bottom_x, hi_size[1]), (0, hi_size[1])], fill=255)
    mask = mask.resize(size, RESAMPLE)
    right.paste(left, (0, 0), mask)
    right.save(out_path, optimize=True)


diagonal('.scratch/screenshots/desktop-light.png', '.scratch/screenshots/desktop-dark.png', '.scratch/screenshots/desktop-light-composite.png', (1860, 1240), 0.86)
diagonal('.scratch/screenshots/mobile-light.png', '.scratch/screenshots/mobile-dark.png', '.scratch/screenshots/mobile-light-composite.png', (564, 1120), 0.80)
diagonal('.scratch/screenshots/desktop-dark.png', '.scratch/screenshots/desktop-light.png', '.scratch/screenshots/desktop-dark-composite.png', (1860, 1240), 0.86)
diagonal('.scratch/screenshots/mobile-dark.png', '.scratch/screenshots/mobile-light.png', '.scratch/screenshots/mobile-dark-composite.png', (564, 1120), 0.80)
PY

cat > .scratch/screenshots/contact-sheet.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>missing-ebooks screenshots</title>
<style>
  * { box-sizing: border-box; }
  :root {
    --matte: #000;
  }
  html,
  body {
    margin: 0;
    width: 2560px;
    height: 1320px;
    overflow: hidden;
    background: var(--matte);
  }
  body {
    display: grid;
    place-items: center;
  }
  main {
    width: 2480px;
    height: 1240px;
    position: relative;
  }
  .device {
    position: absolute;
    margin: 0;
    overflow: hidden;
  }
  .desktop {
    left: 0;
    top: 0;
    width: 1860px;
    height: 1240px;
    border-radius: 60px;
  }
  .mobile {
    right: 0;
    top: 60px;
    width: 564px;
    height: 1120px;
    border-radius: 56px;
  }
  .screen {
    width: 100%;
    height: 100%;
    overflow: hidden;
    background: #101827;
  }
  .desktop .screen {
    border-radius: 60px;
  }
  .mobile .screen {
    border-radius: 56px;
  }
  img {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
<main>
  <figure class="device desktop" aria-label="Desktop screenshot split diagonally">
    <div class="screen"><img id="desktop" src="desktop-light-composite.png" alt=""></div>
  </figure>
  <figure class="device mobile" aria-label="Mobile screenshot split diagonally">
    <div class="screen"><img id="mobile" src="mobile-light-composite.png" alt=""></div>
  </figure>
</main>
HTML

python3 -m http.server 13381 --directory .scratch/screenshots &
playwright-cli open http://127.0.0.1:13381/contact-sheet.html
playwright-cli resize 2560 1320
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-light-black.png
playwright-cli eval "() => document.documentElement.style.setProperty('--matte', '#fff')"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-light-white.png
playwright-cli eval "() => { document.documentElement.style.setProperty('--matte', '#000'); document.getElementById('desktop').src = 'desktop-dark-composite.png'; document.getElementById('mobile').src = 'mobile-dark-composite.png'; }"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-dark-black.png
playwright-cli eval "() => document.documentElement.style.setProperty('--matte', '#fff')"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-dark-white.png
python3 - <<'PY'
from PIL import Image


def alpha_from_pair(black_path, white_path, out_path):
    black = Image.open(black_path).convert('RGB')
    white = Image.open(white_path).convert('RGB')
    out = Image.new('RGBA', black.size)
    out_pix = out.load()
    for y in range(black.height):
        for x in range(black.width):
            br, bg, bb = black.getpixel((x, y))
            wr, wg, wb = white.getpixel((x, y))
            a = 255 - max(wr - br, wg - bg, wb - bb)
            if a <= 0:
                out_pix[x, y] = (0, 0, 0, 0)
            else:
                out_pix[x, y] = (
                    min(255, round(br * 255 / a)),
                    min(255, round(bg * 255 / a)),
                    min(255, round(bb * 255 / a)),
                    a,
                )
    out.save(out_path, optimize=True)


alpha_from_pair('.scratch/screenshots/contact-sheet-light-black.png', '.scratch/screenshots/contact-sheet-light-white.png', 'assets/screenshot-light.png')
alpha_from_pair('.scratch/screenshots/contact-sheet-dark-black.png', '.scratch/screenshots/contact-sheet-dark-white.png', 'assets/screenshot-dark.png')
PY
```

## Wire the README image

GitHub supports theme-specific images through `<picture>` and `prefers-color-scheme` media queries. Keep the fallback `img` on the light screenshot so Markdown renderers that ignore `<picture>` still show an image.

```html
<a href="https://demo-missing-ebooks.noahbaculi.com">
  <picture>
    <source
      media="(prefers-color-scheme: dark)"
      srcset="assets/screenshot-dark.png"
    />
    <source
      media="(prefers-color-scheme: light)"
      srcset="assets/screenshot-light.png"
    />
    <img
      src="assets/screenshot-light.png"
      alt="missing-ebooks tree view in light and dark mode on desktop and mobile"
    />
  </picture>
</a>
```
