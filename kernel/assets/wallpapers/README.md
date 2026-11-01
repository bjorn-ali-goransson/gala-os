# Wallpapers

Four desktop pictures, generated for gala on 2026-11-01 at the project
owner's request with OpenAI's image model `gpt-image-2` (1536 x 1024,
quality "high"); see THIRD_PARTY.md. The prompts:

- `dunes`: "A serene desktop wallpaper: smooth, sculpted desert sand dunes
  of the Najd at golden hour, soft long shadows, warm amber and rose tones
  fading into a pale blue sky, minimal and calm, gentle gradients,
  photographic yet clean, no people, no text, no logos"
- `ribbons`: "An elegant abstract desktop wallpaper: luminous translucent
  glass ribbons flowing in soft curves across a deep blue to teal gradient
  background, subtle light glow, smooth depth of field, modern
  operating-system wallpaper aesthetic, original design, no text, no logos"
- `najdi`: "An abstract desktop wallpaper inspired by traditional Najdi
  mud-brick architecture: warm earthen terracotta and sand tones, soft
  geometric triangular crenellation patterns and small triangular windows
  rendered as a minimal layered composition with gentle lighting and
  shadows, calm and modern, no text, no logos"
- `night`: "A tranquil desktop wallpaper: a starry night sky with the Milky
  Way over silhouetted date palms and low desert dunes, deep indigo and
  violet tones with a faint warm glow on the horizon, peaceful and minimal,
  no people, no text, no logos"

The `.qoi` files are the generated PNGs cropped to 16:10 (centred),
scaled to 1280 x 800 (Lanczos) and encoded as QOI by `make_qoi.py`
(Pillow; written from the specification at qoiformat.org):

    python3 -I make_qoi.py dunes.png dunes.qoi 1280 800

The PNGs themselves are not kept. The kernel decodes the files at boot
(`nt/desktop/paint.rs`, `decode_qoi`).
