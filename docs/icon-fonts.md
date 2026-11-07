# Icon fonts

The app draws its icons as text in two embedded fonts (`src/icon_font.rs`):

* `assets/fonts/MaterialIcons-Regular.ttf`: the classic **Material Icons** set (about 2,200 icons).
* `assets/fonts/MaterialSymbols-subset.ttf`: a 1.4 KB static copy of **Material Symbols Outlined** that contains only the
  few icons that do not exist in Material Icons. At the moment: `reset_image` (U+F824, the "Regenerate icon" button).

Both are Apache-2.0 (Google, github.com/google/material-design-icons).

## Adding another Material Symbols icon

1. Get `MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf` (variable font, about 10 MB) from the material-design-icons repository.
2. Find the icon's code point (for example with fontTools: `TTFont(path).getBestCmap()`, the glyph name is the icon name).
3. Re-cut the subset with all code points the app uses (the old ones too):

```python
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer
from fontTools import subset

f = TTFont("MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf")
inst = instancer.instantiateVariableFont(f, {"FILL": 0, "GRAD": 0, "opsz": 24, "wght": 400}, inplace=False)
opts = subset.Options()
opts.layout_features = []
opts.name_IDs = [1, 2, 3, 4, 5, 6, 13, 14]
opts.notdef_outline = True
opts.hinting = False
s = subset.Subsetter(opts)
s.populate(unicodes=[0xF824])          # add the new code points to this list
s.subset(inst)
inst.save("assets/fonts/MaterialSymbols-subset.ttf")
```

4. Add a constant in `icon_font::g` built with `gs('\u{....}')` (not `gl`) and use it like any other glyph:
   `icon(font, g::YOUR_ICON, size, color)`.

Bevy does not fall back from one font to another per character, which is why a glyph says which font it lives in.
