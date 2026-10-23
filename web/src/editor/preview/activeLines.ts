import type { EditorState, SelectionRange, Text } from '@codemirror/state'

/**
 * The line numbers a selection range touches, first to last. A non-empty
 * range that ends at the very start of a line doesn't touch that line.
 */
export function touchedLines(
  doc: Text,
  range: SelectionRange,
): [number, number] {
  const first = doc.lineAt(range.from).number
  let last = doc.lineAt(range.to).number
  if (
    range.to > range.from &&
    last > first &&
    doc.line(last).from === range.to
  ) {
    last--
  }
  return [first, last]
}

/**
 * The lines live preview reveals: every line any selection range touches
 * (main cursor, other cursors, multi-line selections). Sorted, merged
 * `[first, last]` line-number intervals.
 */
export function activeLines(state: EditorState): [number, number][] {
  const spans = state.selection.ranges
    .map((range) => touchedLines(state.doc, range))
    .sort((a, b) => a[0] - b[0])
  const merged: [number, number][] = []
  for (const [first, last] of spans) {
    const prev = merged[merged.length - 1]
    if (prev && first <= prev[1] + 1) prev[1] = Math.max(prev[1], last)
    else merged.push([first, last])
  }
  return merged
}

/** Whether line `n` is in one of the intervals from {@link activeLines}. */
export function isActiveLine(active: [number, number][], n: number): boolean {
  return active.some(([first, last]) => n >= first && n <= last)
}
