# ui/

Static landing page for raggy. Plain HTML and CSS, no build step.

## Preview locally

Any static server will do. For example:

```bash
# Python
python3 -m http.server -d ui 8000

# Node
npx serve ui

# Just open the file
open ui/index.html
```

Then visit <http://localhost:8000>.

## What's here

- `index.html` — single-page marketing site.
- `styles.css` — the stylesheet. Organised by section; the palette, type
  stack, and scale live in `:root`.

## Typography

The page loads three Google Fonts:

- **Fraunces** — display (headlines, emphasis, wordmark).
- **Inter** — body text and UI.
- **IBM Plex Mono** — labels, code, and metadata strips.

Fraunces is a variable font; the stylesheet uses `font-variation-settings`
(`opsz`, `SOFT`) at different sizes to get an editorial feel at display
sizes and a tighter, more utilitarian one at smaller sizes.

## Editing

The stylesheet is intentionally vanilla CSS. Everything is under
`:root` custom properties at the top of the file — palette, font
stacks, scale, spacing, layout — so retheming doesn't require touching
selectors.

## Deploying

Drop `ui/` behind any static host (GitHub Pages, Netlify, Vercel,
Cloudflare Pages, S3+CloudFront). No server-side anything required.
