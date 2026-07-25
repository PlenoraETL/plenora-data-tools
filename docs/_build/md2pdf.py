# Converte docs/kernel-signatures.md in docs/kernel-signatures.pdf con fpdf2.
# Supporta: #/##/###, paragrafi con **grassetto**/*corsivo*/`codice`,
# pipe table, fenced code block, elenchi "- ".
# Uso: .venv-doc/Scripts/python docs/_build/md2pdf.py
import html
import re
from pathlib import Path

from fpdf import FPDF

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "docs/kernel-signatures.md"
DST = ROOT / "docs/kernel-signatures.pdf"
FONTS = Path("C:/Windows/Fonts")


class Doc(FPDF):
    def header(self):
        if self.page_no() <= 1:
            return
        self.set_font("arial", "I", 8)
        self.set_text_color(120)
        self.cell(0, 5, "plenora-data-tools - Firme dei kernel (v1)", align="R")
        self.ln(7)
        self.set_text_color(0)

    def footer(self):
        self.set_y(-12)
        self.set_font("arial", "", 8)
        self.set_text_color(120)
        self.cell(0, 8, str(self.page_no()), align="C")
        self.set_text_color(0)


def inline(text):
    """Markdown inline -> HTML minimale per write_html."""
    codes = []

    def stash(m):
        codes.append(html.escape(m.group(1), quote=False))
        return f"\x00{len(codes) - 1}\x00"

    t = re.sub(r"`([^`]+)`", stash, text)
    t = html.escape(t, quote=False)
    t = re.sub(r"\*\*([^*]+)\*\*", r"<b>\1</b>", t)
    t = re.sub(r"(?<!\*)\*([^*\n]+)\*(?!\*)", r"<i>\1</i>", t)
    t = re.sub(r"\x00(\d+)\x00",
               lambda m: '<font face="couriernew">' + codes[int(m.group(1))] + "</font>", t)
    return t


def plain(text):
    return re.sub(r"[*`]", "", text)


def main():
    lines = SRC.read_text(encoding="utf-8").splitlines()

    pdf = Doc(format="A4", unit="mm")
    pdf.set_auto_page_break(True, margin=15)
    pdf.set_margins(15, 12, 15)
    pdf.add_font("arial", "", str(FONTS / "arial.ttf"))
    pdf.add_font("arial", "B", str(FONTS / "arialbd.ttf"))
    pdf.add_font("arial", "I", str(FONTS / "ariali.ttf"))
    pdf.add_font("couriernew", "", str(FONTS / "cour.ttf"))
    pdf.add_font("couriernew", "B", str(FONTS / "courbd.ttf"))
    pdf.add_page()
    pdf.set_font("arial", "", 10)

    first_h2_done = False
    i = 0
    while i < len(lines):
        line = lines[i]

        if line.startswith("```"):
            block = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                block.append(lines[i])
                i += 1
            i += 1
            pdf.set_fill_color(243, 243, 243)
            pdf.set_font("couriernew", "", 8.5)
            for bl in block:
                pdf.multi_cell(0, 4.2, bl if bl else " ", fill=True,
                               new_x="LMARGIN", new_y="NEXT")
            pdf.set_font("arial", "", 10)
            pdf.ln(2)
            continue

        if line.startswith("|"):
            rows = []
            while i < len(lines) and lines[i].startswith("|"):
                cells = [c.strip() for c in lines[i].strip().strip("|").split("|")]
                if not all(re.fullmatch(r":?-{2,}:?", c) for c in cells):
                    rows.append([plain(c) for c in cells])
                i += 1
            if rows:
                ncol = max(len(r) for r in rows)
                rows = [r + [""] * (ncol - len(r)) for r in rows]
                weights = []
                for c in range(ncol):
                    w = max(len(r[c]) for r in rows)
                    weights.append(min(max(w, 8), 60))
                pdf.set_font("arial", "", 8.5)
                with pdf.table(
                    col_widths=weights,
                    headings_style=__import__("fpdf").fonts.FontFace(
                        emphasis="BOLD", fill_color=(225, 232, 240)),
                    line_height=4.4,
                    text_align="LEFT",
                    width=180,
                ) as table:
                    for row in rows:
                        tr = table.row()
                        for cell in row:
                            tr.cell(cell)
                pdf.set_font("arial", "", 10)
                pdf.ln(2)
            continue

        if line.startswith("### "):
            pdf.ln(2)
            pdf.set_font("arial", "B", 11.5)
            pdf.set_text_color(20, 50, 110)
            pdf.multi_cell(0, 5.5, plain(line[4:]))
            pdf.set_text_color(0)
            pdf.set_font("arial", "", 10)
        elif line.startswith("## "):
            if first_h2_done:
                pdf.add_page()
            else:
                first_h2_done = True
                pdf.ln(4)
            pdf.set_font("arial", "B", 14)
            pdf.set_text_color(10, 35, 90)
            pdf.multi_cell(0, 7, plain(line[3:]))
            pdf.set_text_color(0)
            pdf.set_draw_color(10, 35, 90)
            pdf.line(pdf.l_margin, pdf.get_y(), pdf.w - pdf.r_margin, pdf.get_y())
            pdf.ln(2)
            pdf.set_font("arial", "", 10)
        elif line.startswith("# "):
            pdf.set_font("arial", "B", 18)
            pdf.multi_cell(0, 9, plain(line[2:]))
            pdf.ln(2)
            pdf.set_font("arial", "", 10)
        elif line.startswith("- "):
            pdf.write_html("<br>&bull;&nbsp;" + inline(line[2:]))
        elif line.strip():
            para = [line]
            while i + 1 < len(lines):
                nxt = lines[i + 1]
                if not nxt.strip() or nxt.startswith(("#", "|", "```", "- ")):
                    break
                para.append(nxt)
                i += 1
            pdf.write_html(inline(" ".join(para)) + "<br>")
        else:
            pdf.ln(1.5)
        i += 1

    pdf.output(str(DST))
    print(f"OK: {DST} ({pdf.page_no()} pagine)")


if __name__ == "__main__":
    main()
