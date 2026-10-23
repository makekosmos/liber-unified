# Reading text and PDF correspondence

The previous mode conversion used paragraph byte counts as an estimate of the
vertical location on a PDF page. It now retains a compact source-line map
(UTF-8 ranges and PDF vertical bounds) and uses the visible shaped text line
when leaving reading mode. Entering reading mode first locates the source
paragraph and then positions its measured text line. Returning without scrolling
preserves the original viewport. Background vocabulary repair also preserves
the list paragraph and offset when replacing the currently visible page.

PDF selections now support Command/Ctrl+C. Both modes expose a Copy button.
Validated PDF copy by pasting its result into the app's local search field.

Reflow distinguishes code using monospace fonts, syntax-bearing lines and
multi-line numeric examples. It preserves newlines, reconstructs indentation,
joins PDF fragments on the same baseline, and renders code in Menlo with a
separate background and horizontal scrolling. Code bypasses prose word repair.
This is heuristic detection, not a parser or a guarantee of valid source code.

Mixed-script single-letter runs no longer merge adjacent Russian words into
English identifiers. Space-fragment repair recognizes common short keywords.
Mixed Latin/Cyrillic lookalikes are corrected conservatively; the `ШЕЕ 754`
correction specifically requires the adjacent IEEE standard number. Pure
Cyrillic prose is not transliterated. The text cache is now `Liber/pdf-v4`.

The book's embedded OCR itself contains `ШЕЕ`, corrupted numeric literals and
flattened mathematical exponents. Its character boxes/font sizes do not encode
the visible superscripts reliably. General recovery of these errors needs a
fresh OCR pass against the page image; this update does not add such a pass or
claim lossless mathematical/code reconstruction.

Validation: 49 tests passed, one network test ignored. A separate test against
the real 732-page JavaScript PDF validated IEEE/keyword repair, UTF-8 source
ranges, and code extraction on pages 50–53. Installed release checked through
PDF/reflow switching at section 3.2, native PDF copy/paste and code rendering.
The app bundle was replaced and its code signature verified.

## Selection and layout follow-up

- Both PDF glyph selection and reflow UTF-8 selection support double-click token selection (letters, digits, underscore and `$`; punctuation remains separate).
- Selection actions use a color row and an accessible icon-only copy button beneath it. Copy dismisses the popup and displays a bottom toast for 1.5 seconds, animating only entry and exit. Two timed wakes avoid an idle animation loop.
- Code text has intrinsic monospace width inside a horizontal scroll viewport; line breaks and indentation stay intact.
- Numeric OCR repairs require a standalone literal plus a matching `// => decimal` result. `0Ы0101` becomes `0b10101` only when it evaluates to the annotated 21; octal and hexadecimal lookalikes follow the same check. Text cache version is v7.
- Contents uses variable-height virtualized rows with normal word wrapping and full-width click targets.
- Numbered PDF headings retain their depth through H3. Non-numbered headings use PDF font size. Reader H1/H2/H3 scales are 1.802/1.602/1.424, with 2.4/1.9/1.5 em top spacing. These are chosen reader settings, not a claim of exact Apple Books parity.

Validation: 55 unit tests passed, one network test ignored. The real JavaScript PDF test additionally verifies corrected binary and octal examples and syntax tokens.

GUI verification: contents wraps `Пример: гистограмма частоты использования символов` and the Unicode heading by whole words; hover and navigation cover full row width. Double-click selected `программу` in PDF; copy closed the popup and showed the bottom toast; pasting into book search yielded exactly that word. Reflow double-click selected the complete `0b10101` token. H3 recognition is also asserted against the real PDF, including its body-sized heading font. Horizontal input automation on this machine emits zero horizontal wheel delta, so wheel gestures cannot be certified through that API; explicit horizontal scrollbar supports pointer dragging. No FPS benchmark was performed.

Final GUI check: dragging the horizontal scrollbar exposed the complete trailing `+ 1*1)` expression while the surrounding prose stayed stationary. The installed release was signed and verified.
