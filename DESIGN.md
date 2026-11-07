---
name: Deskpost site
description: Warm-ink, pixel-typed landing and docs site where loose mail coalesces into an envelope, a thread, and a ticket in the queue.
colors:
  accent: "#ffb020"
  accent-ink: "#0a0908"
  ink: "#0a0908"
  ink-raised: "#12100d"
  ink-sunken: "#1c1814"
  paper: "#f1ebe0"
  paper-dim: "#a99f90"
  hairline: "#26211b"
  hairline-strong: "#3d362c"
typography:
  display:
    fontFamily: "Geist Pixel Square, ui-monospace, SF Mono, monospace"
    fontSize: "clamp(2.5rem, 7.2vw, 6rem)"
    fontWeight: 400
    lineHeight: 1.04
    letterSpacing: "0"
  headline:
    fontFamily: "Geist Pixel Square, ui-monospace, SF Mono, monospace"
    fontSize: "clamp(2rem, 4.4vw, 3.5rem)"
    fontWeight: 400
    lineHeight: 1.04
    letterSpacing: "0"
  title:
    fontFamily: "Geist Pixel Square, ui-monospace, SF Mono, monospace"
    fontSize: "clamp(1.25rem, 2vw, 1.625rem)"
    fontWeight: 400
    lineHeight: 1.2
    letterSpacing: "0"
  body:
    fontFamily: "Satoshi, system-ui, -apple-system, Segoe UI, sans-serif"
    fontSize: "1.0625rem"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "Geist Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "0.8125rem"
    fontWeight: 400
    letterSpacing: "0.04em"
  code:
    fontFamily: "Geist Mono, ui-monospace, SF Mono, Menlo, monospace"
    fontSize: "0.8125rem"
    fontWeight: 400
    lineHeight: 1.75
rounded:
  none: "0"
spacing:
  gutter: "clamp(16px, 4vw, 48px)"
  section: "clamp(64px, 10vw, 128px)"
  nav-height: "64px"
  max-width: "1240px"
components:
  button-default:
    backgroundColor: "transparent"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
    height: "48px"
    padding: "0 20px"
  button-default-hover:
    backgroundColor: "{colors.ink-sunken}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    rounded: "{rounded.none}"
    height: "48px"
    padding: "0 20px"
  button-primary-hover:
    backgroundColor: "{colors.paper}"
  button-ink:
    backgroundColor: "{colors.accent-ink}"
    textColor: "{colors.accent}"
    rounded: "{rounded.none}"
    height: "48px"
    padding: "0 20px"
  copy-line:
    backgroundColor: "{colors.ink-raised}"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
  code-panel:
    backgroundColor: "{colors.accent-ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.none}"
    padding: "20px"
---

# Design System: Deskpost site

## Overview

**Creative North Star: "The Shared Mailbox"**

The site is a well-kept shared mailbox in the dark: brown-leaning ink ground, warm paper text, 1px hairlines, square corners, pixel-grid display type. Nothing is lost and every thread is in order. The hero proves the product by showing `@ > -` glyphs drifting in, coalescing into an envelope that opens, a thread weaving down to the queue, and a ticket row landing in the stack. The rest of the page is receipts, rows, and rules rather than cards and tiles.

One saturated field per page carries the brand color at scale: the install band, drenched in signal amber with ink type. Elsewhere the accent is a sparse mark (a link, an icon, a focus ring, a node, the "post" half of the wordmark). Dark only; there is no light theme. Greys lean warm so nothing reads as cold blue-grey.

The UI kit (`site/src/kit/`) is shared with sibling sites (bunfiiliate in acid green, LearnPuppy in pink). `site/src/kit/kit.css`, `docs.css`, and the type and layout are byte-identical across brands; Deskpost differs only in `site/src/brand.css` (palette values plus three small layout tweaks) and `site/src/scenes/inbox.js`. This document records the Deskpost skin; kit structure must not be forked per brand.

**Key Characteristics:**
- Pixel display type over a quiet humanist body; mono only for code, figures, and facts.
- Square corners everywhere; separation by 1px hairlines and tonal steps, never shadows.
- One drenched amber band; sparse accent marks elsewhere.
- Receipt forms (dashed rows, dotted leaders) instead of icon-card grids.
- Canvas is decoration only: aria-hidden, reduced-motion renders a still frame, all copy is real text.

## Colors

A warm-ink palette with paper-cream text and a single signal-amber accent.

### Primary
- **Signal Amber** (`accent`, #ffb020): the install band field, primary buttons, prose links, list markers, feature icons, focus ring, selection, the "post" in the wordmark, ASCII canvas tone 2 (envelope flap, the `@` seal).
- **Ink on Amber** (`accent-ink`, #0a0908): text, borders, and code panel on the amber field; text on primary buttons.

### Neutral
- **Warm Ink** (`ink`, #0a0908): page ground and nav.
- **Raised Ink** (`ink-raised`, #12100d): copy line, footer, steps band, doc code blocks, blockquotes, search dialog.
- **Sunken Ink** (`ink-sunken`, #1c1814): button hover, inline code chips.
- **Paper** (`paper`, #f1ebe0; token `--fg`): body text, headings, canvas tone 1 (envelope outline).
- **Dim Paper** (`paper-dim`, #a99f90; token `--fg-dim`): ledes, secondary copy, inactive nav links, canvas tone 0 (thread, ambient glyphs).
- **Hairline** (`hairline`, #26211b; token `--line`): section and row dividers.
- **Strong Hairline** (`hairline-strong`, #3d362c; token `--line-2`): control borders, receipt dashes, scrollbar thumbs.

### Named Rules
**The One Field Rule.** Exactly one saturated amber field per page, the install band. Everything else uses the accent as a mark, never as a second block of color.

**The Skin Seam Rule.** A brand skin changes only `--accent`, `--accent-ink`, `--bg*`, `--fg*`, `--line*` in `brand.css`. Never hardcode a hex in component CSS; read the token (the canvas reads tones from computed custom properties).

## Typography

**Display Font:** Geist Pixel Square (fallback ui-monospace, SF Mono)
**Body Font:** Satoshi 400/500/700 (fallback system-ui)
**Label/Mono Font:** Geist Mono 400 (fallback ui-monospace, Menlo)

**Character:** A bitmap-cut display face gives pixel-grid precision, echoing the ASCII scene; Satoshi keeps long reading calm. Mono is reserved for things that are literally code or figures.

### Hierarchy
- **Display** (400, clamp(2.5rem, 7.2vw, 6rem), 1.04): hero h1, capped at 13ch for Deskpost (kit default 11ch; "Support inbox / you host / yourself." needs the wider measure), accent `em` for the emphasized word.
- **Headline** (400, clamp(2rem, 4.4vw, 3.5rem), 1.04): section h2 (`.display`).
- **Title** (400, clamp(1.25rem, 2vw, 1.625rem), 1.2): group heads; flow step h3 at 1.375rem; doc h1 clamp(2.25rem, 4vw, 3.25rem), doc h2 1.75rem.
- **Body** (400, 1.0625rem, 1.6): prose; ledes in dim paper at clamp(1.0625rem, 1.5vw, 1.25rem); doc prose 72ch; row titles Satoshi 700 at 1.0625rem.
- **Label** (Geist Mono 400, 0.8125rem, 0.04em): receipt keys (uppercase), hero meta, star count, footer end line. Buttons and tabs are Satoshi 700 at about 0.9375rem.

### Named Rules
**The Mono Is Literal Rule.** Geist Mono appears only for code, commands, figures, and facts; never for decorative headings or labels that are not data.

**The Pixel Is Display Rule.** The pixel face is for headings and the wordmark, always weight 400 and letter-spacing 0; body text is never pixel.

## Layout

Single column inside `.wrap` (max 1240px, gutter clamp(16px, 4vw, 48px)), sections separated by 1px hairlines with vertical padding clamp(64px, 10vw, 128px). Asymmetric two-column grids collapse to one column at 860px; the nav collapses to a toggle menu at 760px. The hero holds the copy left and the canvas scene right; on mobile the scene sits below the copy, and Deskpost reserves `min(96vw, 400px)` of bottom padding under the hero copy for it. The hero copy block (`x-copy`) is widened to 660px. Docs use a 240px sidebar, fluid article, and 200px TOC, dropping the TOC at 1100px and the sidebar into a toggle at 760px. Spacing rhythm is 4/8 steps (8, 12, 14, 16, 20, 28, 32) inside components.

## Elevation & Depth

Flat. There are no box-shadows. Depth is tonal (ink, raised ink, sunken ink) plus 1px hairlines and the one amber band. Hover is a border or fill change, active is a 1px downward nudge.

### Named Rules
**The Hairline Depth Rule.** Separate surfaces with a 1px line or a tonal step. No shadows, blur, or glass.

## Shapes

Square. Radius is 0 on buttons, inputs, code, tabs, dialogs, chips, and nodes. Borders are 1px solid hairline-strong (controls) or hairline (dividers); receipts use dashed and dotted 1px lines. The wordmark may carry a 22px pixelated mark image beside it. Recurring silhouette: the square timeline node and the segmented tablist with hard 1px dividers.

## Components

### Buttons
- **Shape:** Square, 48px tall (40px in nav), 1px border, 20px inline padding, Satoshi 700.
- **Default:** transparent, hairline-strong border; hover brightens border to paper and fills sunken ink; active nudges 1px down.
- **Primary:** amber fill, ink text; hover flips to paper fill.
- **Ink:** ink fill with amber text, used only on the amber band; hover goes transparent with ink text.
- **Motion:** 0.2s on `cubic-bezier(0.16, 1, 0.3, 1)`.

### Copy line and code panels
Raised ink box with an amber `$` prompt, Geist Mono, and a 48px square copy button that fills amber on hover. On the amber band the code panel is ink with a tablist above it (selected tab inverts to ink fill, amber text). Install commands are real, from `brand.js` (Docker and binary variants).

### Navigation
Sticky, ink ground, 64px, 1px bottom hairline. Wordmark `desk` + `post` in the pixel face, "post" in amber; links Satoshi 500, dim paper, paper on hover or `aria-current`. Docs sidebar marks the current page with amber text and a 1px left rule.

### Receipt (live facts)
Mono list with dashed top and bottom rules per row, uppercase dim key, dotted leader, right-aligned paper value. Figures come from GitHub or repo files; a figure is hidden rather than invented.

### Feature rows, flow, FAQ
Feature rows pair a Hugeicons icon (amber) with a bold title and dim copy. Flow is a vertical hairline with square nodes. FAQ uses native `details`.

### ASCII inbox canvas (signature)
Full-bleed `x-ascii` canvas, aria-hidden, three tones read from tokens (dim paper, paper, amber). One 16 s cycle: `@ > -` glyphs drift in on springs, coalesce into an envelope outline (`+ - |`) that opens, a `| + L` thread weaves down to a queue, and a ticket row (subject, status pending / in_progress / resolved) lands in the stack at bottom right. Ambient glyphs fade toward the headline side so copy stays calm. Pre-stepped frames end on an open envelope, which is the `prefers-reduced-motion` still frame. Hero copy rises in once on load; no other entrance motion.

## Do's and Don'ts

### Do:
- **Do** keep every corner square and every divider a 1px hairline.
- **Do** use Hugeicons through the kit icon set for all icons.
- **Do** keep real numbers only (live GitHub counts, measured figures with their hardware) and hide a figure rather than invent it.
- **Do** keep text in real DOM; the canvas is decorative and aria-hidden.
- **Do** keep a 2px accent focus ring with 3px offset (ink on the amber band).
- **Do** re-skin only through `brand.css` tokens, and state the real repo and binary names (`sendesk`, `api`) where Deskpost is mentioned in docs.

### Don't:
- **Don't** add shadows, glass, decorative gradients, or rounded corners; the only gradient is the hero legibility scrim.
- **Don't** introduce a second saturated field or a light theme.
- **Don't** build icon-card grids, hero metric tiles, fake logos, invented customer counts, or response-time promises.
- **Don't** use Geist Mono or the pixel face for body copy.
- **Don't** hardcode brand colors in kit CSS or JS, or draw chat-bubble clip art in the scene.

## Not canonized

None found in the sampled files (tokens.css, kit.css, docs.css, brand.css, brand.js, scenes/inbox.js): no kickers, shadows, or glyph icons were observed. The kit's FAQ "+" glyph and one-off fractional doc sizes (0.9688rem, 0.8438rem) are inherited drift and not rules. The scene uses typed ASCII glyphs as its canvas material, which is the signature itself, not an icon.
