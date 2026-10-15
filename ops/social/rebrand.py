#!/usr/bin/env python3
"""Rebrand a bluesky-social/social-app checkout as W3BS Social, in the w3bs.org design.

    python3 ops/social/rebrand.py /path/to/social-app

Then build it with social-app's own Dockerfile (see README.md). Every step fails loudly
when upstream moves the code it patches, so a rebuild never ships half a rebrand.

What changes: the default PDS (social.w3bs.org), the colour palette (w3bs paper, ink and
orange; dark and dim are derived by inverting it), the logo components (the w3bs orbit
mark and the "w3bs." wordmark), the welcome dialog (paper instead of the sky photo, a
Georgia headline), page titles, favicons and the default social card.
"""
import pathlib
import re
import shutil
import sys

ROOT = pathlib.Path(sys.argv[1]).resolve()
BRAND = pathlib.Path(__file__).resolve().parent / "brand"
PDS = "https://social.w3bs.org"
ORANGE, PAPER, INK = "#DF5836", "#FFFAF0", "#23352F"


def patch(rel, old, new, count=1):
    path = ROOT / rel
    text = path.read_text()
    found = text.count(old)
    if found != count:
        sys.exit(f"{rel}: expected {count} of {old[:70]!r}, found {found}; upstream changed")
    path.write_text(text.replace(old, new))


def write(rel, text):
    (ROOT / rel).write_text(text.lstrip("\n"))


# 1. Sign in and sign up default to our PDS.
patch(
    "src/lib/constants.ts",
    "export const DEFAULT_SERVICE = BSKY_SERVICE",
    f"export const DEFAULT_SERVICE = '{PDS}'",
)

# 2. Palette. contrast_* runs paper -> ink (contrast_0 is the page), primary_* is the
# w3bs orange (#D95231 at 500). Status colours keep Bluesky's values.
PALETTE = """
import {createThemes, DEFAULT_PALETTE, type Palette} from '@bsky.app/alf'

// W3BS Social: the w3bs.org palette (paper, ink, orange). Dark and dim are inverted
// from it by createThemes.
const W3BS_PALETTE: Palette = {
  ...DEFAULT_PALETTE,
  contrast_0: '#FBFAF5',
  contrast_25: '#F6F4ED',
  contrast_50: '#EEECE2',
  contrast_100: '#DBDED0',
  contrast_200: '#C5C9B9',
  contrast_300: '#ABB0A0',
  contrast_400: '#8E9487',
  contrast_500: '#6B7268',
  contrast_600: '#576058',
  contrast_700: '#45504A',
  contrast_800: '#36433D',
  contrast_900: '#23352F',
  contrast_950: '#1B2925',
  contrast_975: '#131E1B',
  contrast_1000: '#0B1210',
  primary_25: '#FEF7F4',
  primary_50: '#FDEDE7',
  primary_100: '#FADACF',
  primary_200: '#F5BBA8',
  primary_300: '#EE9478',
  primary_400: '#E5704F',
  primary_500: '#D95231',
  primary_600: '#BB4127',
  primary_700: '#993420',
  primary_800: '#78291A',
  primary_900: '#571E13',
  primary_950: '#43170F',
  primary_975: '#2F100A',
}

const DEFAULT_THEMES = createThemes({
  defaultPalette: W3BS_PALETTE,
  subduedPalette: W3BS_PALETTE,
})
"""
themes = (ROOT / "src/alf/themes.ts").read_text()
head = re.compile(r"\Aimport \{\s*createThemes,.*?\n\}\)\n", re.S)
if not head.search(themes):
    sys.exit("src/alf/themes.ts: createThemes header not found; upstream changed")
(ROOT / "src/alf/themes.ts").write_text(head.sub(PALETTE.lstrip("\n"), themes, count=1))

# 3. Logos: the orbit mark (public/mark.svg in profullstack/w3bs.org) and the wordmark.
MARK = f"""
      <Rect width={{64}} height={{64}} rx={{16}} fill={{fill}} />
      <G fill="none" stroke="{PAPER}" strokeWidth={{3.2}}>
        <Ellipse cx={{32}} cy={{32}} rx={{22}} ry={{11}} transform="rotate(-30 32 32)" />
        <Ellipse cx={{32}} cy={{32}} rx={{22}} ry={{11}} transform="rotate(30 32 32)" />
        <Path d="M32 10v44" />
      </G>
      <Circle cx={{32}} cy={{32}} r={{5}} fill="{PAPER}" />"""
WORD = """
        <TSpan>w3bs</TSpan>
        <TSpan fill="#D95231">.</TSpan>"""

write("src/view/icons/Logo.tsx", f"""
import {{forwardRef}} from 'react'
import {{type TextProps}} from 'react-native'
import Svg, {{
  Circle,
  Ellipse,
  G,
  Path,
  type PathProps,
  Rect,
  type SvgProps,
}} from 'react-native-svg'

import {{flatten}} from '#/alf'

// W3BS Social: the w3bs.org orbit mark. "sky" (the old gradient) means the brand orange.
type Props = {{
  allowVariants?: boolean
  fill?: PathProps['fill']
  style?: TextProps['style']
}} & Omit<SvgProps, 'style'>

export const Logo = forwardRef(function LogoImpl(props: Props, ref) {{
  const {{allowVariants: _allowVariants, fill: rawFill, ...rest}} = props
  const styles = flatten(props.style)
  const fill = !rawFill || rawFill === 'sky' ? '{ORANGE}' : rawFill
  const size = parseInt(String(rest.width || 32), 10)
  return (
    <Svg
      fill="none"
      // @ts-ignore forwarded ref
      ref={{ref}}
      viewBox="0 0 64 64"
      {{...rest}}
      accessibilityLabel="W3BS Social"
      style={{[{{width: size, height: size}}, styles]}}>{MARK}
    </Svg>
  )
}})
""")

write("src/view/icons/Logomark.tsx", f"""
import Svg, {{
  Circle,
  Ellipse,
  G,
  Path,
  type PathProps,
  Rect,
  type SvgProps,
}} from 'react-native-svg'

// W3BS Social: the w3bs.org orbit mark, always in the brand orange.
export function Logomark({{
  fill: _fill,
  ...rest
}}: {{fill?: PathProps['fill']}} & SvgProps) {{
  const fill = '{ORANGE}'
  const size = parseInt(String(rest.width || 32), 10)
  return (
    <Svg fill="none" viewBox="0 0 64 64" {{...rest}} width={{size}} height={{size}}>{MARK}
    </Svg>
  )
}}
""")

write("src/view/icons/Logotype.tsx", f"""
import Svg, {{type PathProps, type SvgProps, Text, TSpan}} from 'react-native-svg'

import {{usePalette}} from '#/lib/hooks/usePalette'

// W3BS Social: the "w3bs." wordmark (w3bs.org's .wordmark: heavy sans, tight tracking).
const ratio = 17 / 46

export function Logotype({{
  fill,
  ...rest
}}: {{fill?: PathProps['fill']}} & SvgProps) {{
  const pal = usePalette('default')
  const size = parseInt(String(rest.width || 32), 10)
  return (
    <Svg
      fill="none"
      viewBox="0 0 46 17"
      {{...rest}}
      width={{size}}
      height={{size * ratio}}>
      <Text
        x={{0}}
        y={{14}}
        fontSize={{18}}
        fontWeight="800"
        letterSpacing={{-1}}
        fontFamily="Arial, Helvetica, sans-serif"
        fill={{fill || pal.text.color}}>{WORD}
      </Text>
    </Svg>
  )
}}
""")

write("src/view/icons/LogomarkWithType.tsx", f"""
import Svg, {{
  Circle,
  Ellipse,
  G,
  Path,
  type PathProps,
  Rect,
  type SvgProps,
  Text,
  TSpan,
}} from 'react-native-svg'

import {{useTheme}} from '#/alf'

// W3BS Social: orbit mark + "w3bs." wordmark.
const ratio = 31 / 112

export function LogomarkWithType({{
  fill: textFill,
  ...rest
}}: {{fill?: PathProps['fill']}} & SvgProps) {{
  const t = useTheme()
  const fill = '{ORANGE}'
  const size = parseInt(`${{rest.width || 32}}`, 10)
  return (
    <Svg
      fill="none"
      viewBox="0 0 112 31"
      {{...rest}}
      width={{size}}
      height={{size * ratio}}>
      <G transform="scale(0.484375)">{MARK}
      </G>
      <Text
        x={{38}}
        y={{24}}
        fontSize={{30}}
        fontWeight="800"
        letterSpacing={{-1.5}}
        fontFamily="Arial, Helvetica, sans-serif"
        fill={{textFill || t.atoms.text.color}}>{WORD}
      </Text>
    </Svg>
  )
}}
""")

# 4. Welcome dialog: paper instead of the sky photo, the wordmark, a Georgia headline.
W = "src/components/WelcomeModal.tsx"
patch(W, "const welcomeModalBg = require('../../assets/images/welcome-modal-bg.jpg')", "const welcomeModalBg = null")
patch(W, "backgroundColor: '#C0DCF0',", "backgroundColor: '#F6F4ED',")
patch(
    W,
    """                      {color: '#354358', letterSpacing: -0.5},
                    ]}>
                    Bluesky
                  </Text>""",
    f"""                      {{color: '{INK}', letterSpacing: -1, fontWeight: '800'}},
                    ]}}>
                    w3bs<Text style={{{{color: '#D95231'}}}}>.</Text> social
                  </Text>""",
)
patch(
    W,
    """                      backgroundImage:
                        'linear-gradient(180deg, #313F54 0%, #667B99 83.65%, rgba(102, 123, 153, 0.50) 100%)',
                      backgroundClip: 'text',
                      WebkitBackgroundClip: 'text',
                      WebkitTextFillColor: 'transparent',
                      color: 'transparent',
                      lineHeight: 1.2,
                      letterSpacing: -0.5,""",
    f"""                      fontFamily: "Georgia, 'Times New Roman', serif",
                      fontWeight: '400',
                      color: '{INK}',
                      lineHeight: 1.15,
                      letterSpacing: -1,""",
)
patch(W, "{color: '#354358'},", f"{{color: '{INK}'}},")
# The themed Text forces the app font (applyFonts), so the headline is a plain RN Text,
# set like the w3bs.org hero: Georgia, ink, the last line in orange italic.
patch(W, "import {Pressable, View} from 'react-native'", "import {Pressable, Text as RNText, View} from 'react-native'")
patch(
    W,
    """                <Text
                  style={[
                    gtMobile ? a.text_4xl : a.text_3xl,
                    a.font_semi_bold,
                    a.text_center,
                    {color: '#23352F'},
                    web({
                      fontFamily: "Georgia, 'Times New Roman', serif",
                      fontWeight: '400',
                      color: '#23352F',
                      lineHeight: 1.15,
                      letterSpacing: -1,
                    }),
                  ]}>
                  <Trans>Real people.</Trans>
                  {'\\n'}
                  <Trans>Real conversations.</Trans>
                  {'\\n'}
                  <Trans>Social media you control.</Trans>
                </Text>""",
    f"""                <RNText
                  style={{{{
                    fontFamily: "Georgia, 'Times New Roman', serif",
                    fontSize: gtMobile ? 52 : 38,
                    lineHeight: gtMobile ? 56 : 42,
                    letterSpacing: gtMobile ? -2.3 : -1.6,
                    color: '{INK}',
                    textAlign: 'center',
                  }}}}>
                  <Trans>Real people.</Trans>
                  {{'\\n'}}
                  <Trans>Real conversations.</Trans>
                  {{'\\n'}}
                  <RNText style={{{{color: '#D95231', fontStyle: 'italic'}}}}>
                    <Trans>Social media you control.</Trans>
                  </RNText>
                </RNText>""",
)
# Hard-coded Bluesky blue and slate in the dialog's buttons and links.
patch(W, "#006AFF", "#D95231", count=3)
patch(W, "color: '#405168'", "color: '#6B7268'")
patch(W, "color: '#354358'", f"color: '{INK}'")

# 5. Titles.
patch("src/lib/strings/headings.ts", "— Bluesky`", "— W3BS Social`")
for tmpl in sorted((ROOT / "bskyweb/templates").glob("*.html")):
    text = tmpl.read_text()
    text = text.replace("{%- block head_title -%}Bluesky{%- endblock -%}", "{%- block head_title -%}W3BS Social{%- endblock -%}")
    text = text.replace("{% block head_title %}Bluesky{% endblock %}", "{% block head_title %}W3BS Social{% endblock %}")
    text = text.replace(" - Bluesky", " - W3BS Social").replace(" on Bluesky", " on W3BS Social")
    text = text.replace('content="Bluesky Social"', 'content="W3BS Social"').replace('color="#1185fe"', 'color="#d95231"')
    tmpl.write_text(text)

# 6. Icons and the default social card.
for f in BRAND.iterdir():
    shutil.copy(f, ROOT / "bskyweb/static" / f.name)

print(f"rebranded {ROOT} as W3BS Social")
