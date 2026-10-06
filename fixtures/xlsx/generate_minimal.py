"""Generate the synthetic scalar XLSX fixture using only Python's stdlib."""

from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo


ROOT = Path(__file__).parent
PARTS = {
    "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>',
    "_rels/.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r0" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>',
    "xl/workbook.xml": '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="r1"/><sheet name="Plan" sheetId="2" r:id="r2"/></sheets></workbook>',
    "xl/_rels/workbook.xml.rels": '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="r2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="r3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>',
    "xl/sharedStrings.xml": '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>hello &amp; world</t></si></sst>',
    "xl/worksheets/sheet1.xml": '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>-0</v></c><c r="C1" t="b"><v>1</v></c><c r="D1"><f>A1+B1</f><v>999</v></c></row></sheetData></worksheet>',
    "xl/worksheets/sheet2.xml": '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="4"><c r="Z4" t="inlineStr"><is><t>world&#33;</t></is></c></row></sheetData></worksheet>',
}


with ZipFile(ROOT / "minimal_scalars.xlsx", "w") as archive:
    for name, content in sorted(PARTS.items()):
        info = ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
        info.compress_type = ZIP_DEFLATED
        archive.writestr(info, content.encode("utf-8"))

single_sheet_parts = dict(PARTS)
single_sheet_parts["xl/workbook.xml"] = single_sheet_parts["xl/workbook.xml"].replace(
    '<sheet name="Plan" sheetId="2" r:id="r2"/>', ""
)
single_sheet_parts["xl/_rels/workbook.xml.rels"] = single_sheet_parts[
    "xl/_rels/workbook.xml.rels"
].replace(
    '<Relationship Id="r2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>',
    "",
)
single_sheet_parts.pop("xl/worksheets/sheet2.xml")
with ZipFile(ROOT / "single_sheet_scalars.xlsx", "w") as archive:
    for name, content in sorted(single_sheet_parts.items()):
        info = ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
        info.compress_type = ZIP_DEFLATED
        archive.writestr(info, content.encode("utf-8"))

merged_parts = dict(single_sheet_parts)
merged_parts["xl/worksheets/sheet1.xml"] = merged_parts[
    "xl/worksheets/sheet1.xml"
].replace(
    "</worksheet>",
    '<mergeCells count="1"><mergeCell ref="E1:F1"/></mergeCells></worksheet>',
)
with ZipFile(ROOT / "single_sheet_merge.xlsx", "w") as archive:
    for name, content in sorted(merged_parts.items()):
        info = ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
        info.compress_type = ZIP_DEFLATED
        archive.writestr(info, content.encode("utf-8"))

cross_sheet_parts = dict(PARTS)
cross_sheet_parts["xl/worksheets/sheet1.xml"] = '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f>Plan!A1+2</f><v>999</v></c></row></sheetData></worksheet>'
cross_sheet_parts["xl/worksheets/sheet2.xml"] = '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>3</v></c></row></sheetData></worksheet>'
with ZipFile(ROOT / "cross_sheet_formula.xlsx", "w") as archive:
    for name, content in sorted(cross_sheet_parts.items()):
        info = ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
        info.compress_type = ZIP_DEFLATED
        archive.writestr(info, content.encode("utf-8"))
