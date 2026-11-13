"""Build the rat user guide as PDFs, in English and German.

    python tools/user_guide.py [docs]        # -> docs/rat-user-guide-en.pdf, -de.pdf
    python tools/user_guide.py --lang de     # one language only

Needs reportlab (pip install reportlab). guide_lib.py holds fonts, styles and
helpers (the look of the mimic user guide), guide_en.py and guide_de.py the
text. Keep the two in step with each other, with README.md,
docs/rat-syntax.md and the output of `rat --help`, `rat cpus`,
`rat platforms` and `rat formats`.
"""

import importlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import guide_lib as g  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LANGUAGES = ["en", "de"]


def build(lang, out_dir):
    g.reset()
    g.front_matter(lang)
    importlib.import_module("guide_" + lang)
    out = os.path.join(out_dir, "rat-user-guide-%s.pdf" % lang)
    g.Doc(out, lang).multiBuild(list(g.story))
    print("wrote", out)


def main():
    args = sys.argv[1:]
    langs = LANGUAGES
    if "--lang" in args:
        i = args.index("--lang")
        langs = [args[i + 1]]
        del args[i:i + 2]
    out_dir = args[0] if args else os.path.join(ROOT, "docs")
    for lang in langs:
        build(lang, out_dir)


if __name__ == "__main__":
    main()
