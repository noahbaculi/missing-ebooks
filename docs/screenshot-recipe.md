# Recreating `assets/screenshot.png`

Steps to reproduce the README screenshot contact sheet with `playwright-cli` against the `explore` example.

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

## Compose the README image

Build flat diagonal composites first, then place those images inside the final rounded frames. The page is captured once on black and once on white, then Pillow reconstructs alpha from the pair so only the outside canvas becomes transparent.

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


def diagonal(light_path, dark_path, out_path, size):
    light = cover(light_path, size)
    dark = cover(dark_path, size)
    scale = 4
    hi_size = (size[0] * scale, size[1] * scale)
    mask = Image.new('L', hi_size, 0)
    slope = math.tan(math.radians(ANGLE_DEGREES))
    center_x = hi_size[0] / 2
    center_y = hi_size[1] / 2
    top_x = center_x + center_y / slope
    bottom_x = center_x - center_y / slope
    ImageDraw.Draw(mask).polygon([(0, 0), (top_x, 0), (bottom_x, hi_size[1]), (0, hi_size[1])], fill=255)
    mask = mask.resize(size, RESAMPLE)
    dark.paste(light, (0, 0), mask)
    dark.save(out_path, optimize=True)


diagonal('.scratch/screenshots/desktop-light.png', '.scratch/screenshots/desktop-dark.png', '.scratch/screenshots/desktop-composite.png', (1860, 1240))
diagonal('.scratch/screenshots/mobile-light.png', '.scratch/screenshots/mobile-dark.png', '.scratch/screenshots/mobile-composite.png', (564, 1120))
PY

cat > .scratch/screenshots/contact-sheet.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>missing-ebooks screenshots</title>
<style>
  * { box-sizing: border-box; }
  :root {
    --matte: #000;
    --phone: #101827;
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
  <figure class="device desktop" aria-label="Desktop light and dark screenshot split diagonally">
    <div class="screen"><img src="desktop-composite.png" alt=""></div>
  </figure>
  <figure class="device mobile" aria-label="Mobile light and dark screenshot split diagonally">
    <div class="screen"><img src="mobile-composite.png" alt=""></div>
  </figure>
</main>
HTML

python3 -m http.server 13381 --directory .scratch/screenshots &
playwright-cli open http://127.0.0.1:13381/contact-sheet.html
playwright-cli resize 2560 1320
playwright-cli eval "() => { document.activeElement?.blur(); window.getSelection()?.removeAllRanges(); window.scrollTo(0, 0); }"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-black.png
playwright-cli eval "() => document.documentElement.style.setProperty('--matte', '#fff')"
playwright-cli screenshot --filename=.scratch/screenshots/contact-sheet-white.png
python3 - <<'PY'
from PIL import Image
black = Image.open('.scratch/screenshots/contact-sheet-black.png').convert('RGB')
white = Image.open('.scratch/screenshots/contact-sheet-white.png').convert('RGB')
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
out.save('assets/screenshot.png', optimize=True)
PY
```
