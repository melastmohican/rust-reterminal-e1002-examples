#!/usr/bin/env python3
"""
convert_image.py  —  Demo_BMP helper
=====================================
Convert any image to a 24-bit uncompressed BMP sized exactly 800 x 480,
ready to load from an SD card with the Demo_BMP Arduino sketch.

The script optionally quantizes the palette to the 6 e-ink colors so you
can preview exactly what the display will show before copying to the card.

Usage
-----
    # Basic: resize + save as BMP (full color, quantized on the ESP32)
    python3 convert_image.py photo.jpg /Volumes/SD/images/image.bmp

    # Preview mode: show the quantized result on screen without saving
    python3 convert_image.py photo.jpg --preview

    # Save quantized preview as a PNG alongside the BMP
    python3 convert_image.py photo.jpg /Volumes/SD/images/image.bmp --save-preview

Requirements
------------
    pip install Pillow numpy

Supported input formats: JPEG, PNG, WEBP, TIFF, GIF, BMP, and anything
Pillow can decode.
"""

import argparse
import sys
from pathlib import Path

try:
    from PIL import Image
    import numpy as np
except ImportError:
    print("ERROR: Install dependencies first:\n  pip install Pillow numpy", file=sys.stderr)
    sys.exit(1)

# ---------------------------------------------------------------------------
# Panel constants
# ---------------------------------------------------------------------------
PANEL_W = 800
PANEL_H = 480

# The 6 e-ink colors available on the GDEP073E01 panel (no orange).
# Stored as (R, G, B).
PALETTE_RGB = np.array([
    [  0,   0,   0],   # Black
    [255, 255, 255],   # White
    [  0, 255,   0],   # Green
    [  0,   0, 255],   # Blue
    [255,   0,   0],   # Red
    [255, 255,   0],   # Yellow
], dtype=np.float32)


def quantize_to_palette(img_rgb: np.ndarray) -> np.ndarray:
    """Map every pixel to its nearest e-ink palette color (vectorized)."""
    # img_rgb: (H, W, 3) float32
    h, w, _ = img_rgb.shape
    pixels = img_rgb.reshape(-1, 3)               # (N, 3)
    # Squared Euclidean distance from each pixel to each palette entry.
    dists = np.sum(
        (pixels[:, None, :] - PALETTE_RGB[None, :, :]) ** 2,
        axis=2
    )                                              # (N, 6)
    nearest = np.argmin(dists, axis=1)            # (N,)
    quantized = PALETTE_RGB[nearest].astype(np.uint8)
    return quantized.reshape(h, w, 3)


def main():
    parser = argparse.ArgumentParser(
        description="Convert any image to an 800x480 24-bit BMP for Demo_BMP."
    )
    parser.add_argument("input",  help="Source image (JPEG, PNG, etc.)")
    parser.add_argument("output", nargs="?",
                        help="Output .bmp path on the SD card")
    parser.add_argument("--preview",      action="store_true",
                        help="Show quantized preview on screen")
    parser.add_argument("--save-preview", action="store_true",
                        help="Save quantized preview as <output>_preview.png")
    parser.add_argument("--no-quantize",  action="store_true",
                        help="Skip palette quantization (full RGB in BMP)")
    args = parser.parse_args()

    if not args.output and not args.preview:
        parser.error("Provide an output path, --preview, or both.")

    # ------------------------------------------------------------------
    # Load & resize
    # ------------------------------------------------------------------
    src = Path(args.input)
    if not src.exists():
        print(f"ERROR: Input file not found: {src}", file=sys.stderr)
        sys.exit(1)

    print(f"Loading  : {src}")
    img = Image.open(src).convert("RGB")
    print(f"Original : {img.width} x {img.height}")

    # Resize with high-quality Lanczos resampling, preserving aspect ratio
    # by cropping the center (cover-fill behavior).
    img_ratio   = img.width / img.height
    panel_ratio = PANEL_W / PANEL_H

    if img_ratio > panel_ratio:
        # Image is wider — fit height, crop sides
        new_h = PANEL_H
        new_w = int(img.width * PANEL_H / img.height)
        img = img.resize((new_w, new_h), Image.LANCZOS)
        left = (new_w - PANEL_W) // 2
        img = img.crop((left, 0, left + PANEL_W, PANEL_H))
    else:
        # Image is taller — fit width, crop top/bottom
        new_w = PANEL_W
        new_h = int(img.height * PANEL_W / img.width)
        img = img.resize((new_w, new_h), Image.LANCZOS)
        top = (new_h - PANEL_H) // 2
        img = img.crop((0, top, PANEL_W, top + PANEL_H))

    print(f"Resized  : {img.width} x {img.height}")

    # ------------------------------------------------------------------
    # Optional quantization
    # ------------------------------------------------------------------
    if args.no_quantize:
        out_img = img
    else:
        print("Quantizing to 6 e-ink colors ...")
        arr = np.array(img, dtype=np.float32)
        out_arr = quantize_to_palette(arr)
        out_img = Image.fromarray(out_arr, "RGB")

    # ------------------------------------------------------------------
    # Save BMP
    # ------------------------------------------------------------------
    if args.output:
        dst = Path(args.output)
        dst.parent.mkdir(parents=True, exist_ok=True)
        # Save as 24-bit uncompressed BMP (Pillow default for RGB BMP).
        out_img.save(str(dst), format="BMP")
        size_kb = dst.stat().st_size / 1024
        print(f"Saved BMP: {dst}  ({size_kb:.0f} kB)")

    # ------------------------------------------------------------------
    # Preview / save-preview
    # ------------------------------------------------------------------
    if args.save_preview and args.output:
        preview_path = Path(args.output).with_suffix("") \
                       .parent / (Path(args.output).stem + "_preview.png")
        out_img.save(str(preview_path), format="PNG")
        print(f"Preview  : {preview_path}")

    if args.preview:
        print("Showing preview (close window to exit) ...")
        out_img.show()

    print("Done.")


if __name__ == "__main__":
    main()
