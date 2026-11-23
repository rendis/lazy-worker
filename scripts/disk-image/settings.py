# Layout of the installer disk image, for dmgbuild.
# The release command runs it:
#   dmgbuild -s settings.py -D app=<path to the app> -D background=<path to background.png> "Lazy Worker" <output.dmg>
# dmgbuild writes the Finder window settings itself, so the build needs no Finder and no screen.

import os.path

# dmgbuild runs this file without `__file__`, so the caller gives both paths.
application = defines["app"]  # noqa: F821

format = "UDZO"
files = [application]
symlinks = {"Applications": "/Applications"}

# The picture shows two clay pads and an arrow. Each icon stands on its pad, a little above the pad's center.
# dmgbuild adds `background@2x.png` for Retina screens by itself.
background = defines["background"]  # noqa: F821
window_rect = ((200, 200), (660, 400))
icon_locations = {
    os.path.basename(application): (182, 171),
    "Applications": (477, 171),
}

default_view = "icon-view"
icon_size = 128
text_size = 14
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
