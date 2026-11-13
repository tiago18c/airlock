"""Makes the Android app's launcher icon from the mark the desktop app uses.

    python scripts/make-android-icons.py

Reads apps/airlock-mobile/icons/icon.png (the vault door, 512 by 512, the
desktop app's icon) and writes apps/airlock-mobile/android/res/:

  mipmap-anydpi-v26/ic_launcher.xml   the adaptive icon (Android 8 and later,
                                      so every phone the app supports)
  mipmap-<density>/ic_airlock_foreground.webp
                                      its foreground: the door, inside the
                                      safe zone, on a 108 dp canvas
  mipmap-<density>/ic_airlock_monochrome.webp
                                      its monochrome layer, for Android 13's
                                      themed icons: the door's padlock, cut out
                                      of the same art by its glowing lines
  values/airlock_icon.xml             its background: the dark theme's --bg
  mipmap-<density>/ic_launcher.webp   the door alone, for a launcher that does
                                      not draw adaptive icons

scripts/android-icon.sh copies them into the Gradle project dx generates. The
names dx writes there are kept (ic_launcher.webp, mipmap-anydpi-v26/
ic_launcher.xml), so a later dx build overwrites these files rather than
adding a second resource of the same name; the others have names of their own.

Needs Python 3 and Pillow with WebP (python -m pip install pillow). Run it
again after the art changes, and commit what it writes.
"""
import os
import sys

from PIL import Image, ImageFilter, ImageOps

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SOURCE = os.path.join(ROOT, "apps", "airlock-mobile", "icons", "icon.png")
OUT = os.path.join(ROOT, "apps", "airlock-mobile", "android", "res")

# tokens.css, the dark theme's --bg: the colour the app opens on in the dark,
# and close to the door's own glass.
BACKGROUND = "#0C0F15"

# Pixels per dp at each density.
DENSITIES = {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}

# An adaptive icon's layers are 108 dp square. A launcher masks them to a
# shape of its choosing inside the centre 72 dp, and only a circle of 66 dp
# across, the safe zone, is never cut.
CANVAS_DP = 108
SAFE_DP = 66
# The door is a rounded rectangle: at this height its corners sit just inside
# the safe zone's circle, so a round mask never cuts into the frame.
DOOR_DP = 50
# The padlock alone, for the monochrome layer: the door's outline would be a
# plain rounded rectangle in one colour, and the padlock at the size it has in
# the door would be too fine to read.
LOCK_DP = 40
# A legacy icon is 48 dp; the door fills it but for a dp each side.
LEGACY_DP = 48
LEGACY_DOOR_DP = 46


def crop_to_content(im):
    return im.crop(im.getchannel("A").getbbox())


def fit(im, height):
    w, h = im.size
    return im.resize((max(1, round(w * height / h)), height), Image.LANCZOS)


def centred(layer, size):
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    canvas.alpha_composite(layer, ((size - layer.width) // 2, (size - layer.height) // 2))
    return canvas


def padlock_mask(door):
    """The padlock's lines, white on black, at the source's scale."""
    # The middle of the door, where the padlock is: nothing else there is as
    # bright as its glowing lines.
    w, h = door.size
    box = (int(w * 0.32), int(h * 0.28), int(w * 0.68), int(h * 0.62))
    lines = ImageOps.grayscale(door.convert("RGB")).crop(box).point(lambda v: 255 if v > 150 else 0)
    lines = lines.crop(lines.getbbox())
    # Close the glow's double line round the shackle into one stroke, and
    # weight the body's outline so it holds at a launcher's size.
    return lines.filter(ImageFilter.MaxFilter(9)).filter(ImageFilter.MinFilter(3))


def check_corners_inside_safe_zone(door, px_per_dp):
    """Every opaque pixel of the door is within the safe zone's circle."""
    a = door.getchannel("A")
    w, h = door.size
    radius = SAFE_DP / 2 * px_per_dp
    cx, cy = w / 2, h / 2
    px = a.load()
    worst = 0.0
    for y in range(h):
        for x in range(w):
            if px[x, y] > 128:
                d = ((x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2) ** 0.5
                worst = max(worst, d)
    if worst > radius + 1:
        sys.exit(f"the door reaches {worst:.1f} px from the centre, past the safe zone's {radius:.1f}")


def save(im, *parts):
    path = os.path.join(OUT, *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    im.save(path, "WEBP", lossless=True, quality=100, method=6, exact=False)
    return path


def write_text(text, *parts):
    path = os.path.join(OUT, *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


ADAPTIVE = """<?xml version="1.0" encoding="utf-8"?>
<!-- Made by scripts/make-android-icons.py; copied over dx's own by scripts/android-icon.sh. -->
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/airlock_icon_background" />
    <foreground android:drawable="@mipmap/ic_airlock_foreground" />
    <monochrome android:drawable="@mipmap/ic_airlock_monochrome" />
</adaptive-icon>
"""

COLOURS = f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Made by scripts/make-android-icons.py: the launcher icon's background. -->
<resources>
    <color name="airlock_icon_background">{BACKGROUND}</color>
</resources>
"""


def main():
    door = crop_to_content(Image.open(SOURCE).convert("RGBA"))
    lock = padlock_mask(door)

    for name, k in DENSITIES.items():
        canvas = round(CANVAS_DP * k)

        fg_door = fit(door, round(DOOR_DP * k))
        check_corners_inside_safe_zone(fg_door, k)
        save(centred(fg_door, canvas), f"mipmap-{name}", "ic_airlock_foreground.webp")

        # Scale the mask up smooth, then threshold it back to a clean edge.
        mask = fit(lock, round(LOCK_DP * k))
        alpha = mask.point(lambda v: min(255, max(0, (v - 64) * 2)))
        white = Image.new("RGBA", mask.size, (255, 255, 255, 0))
        white.putalpha(alpha)
        save(centred(white, canvas), f"mipmap-{name}", "ic_airlock_monochrome.webp")

        legacy = fit(door, round(LEGACY_DOOR_DP * k))
        save(centred(legacy, round(LEGACY_DP * k)), f"mipmap-{name}", "ic_launcher.webp")

    write_text(ADAPTIVE, "mipmap-anydpi-v26", "ic_launcher.xml")
    write_text(COLOURS, "values", "airlock_icon.xml")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
