/**
 * Turning a key press into the accelerator the backend registers (`F8`, `Ctrl+Shift+P`).
 *
 * The physical `code` names the key, not the character it types, so a layout or a held Shift
 * cannot change which key gets registered.
 */

export type KeyPress = Pick<KeyboardEvent, 'code' | 'key' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>

const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'AltGraph', 'Meta', 'OS'])

/** A key that types in chat. Global, it would fire on every chat message that contains it. */
const TYPING_KEY = /^(Key[A-Z]|Digit\d|Space)$/

export type HotkeyCapture = { accelerator: string } | { problem: string }

/** The accelerator for a press, a reason it cannot be one, or null while only modifiers are down. */
export function hotkeyFromKey(press: KeyPress): HotkeyCapture | null {
  if (MODIFIER_KEYS.has(press.key) || !press.code) return null
  if (TYPING_KEY.test(press.code) && !press.ctrlKey && !press.altKey && !press.metaKey) {
    return { problem: 'Add Ctrl or Alt to a letter, number or Space.' }
  }
  const name = press.code.startsWith('Key') ? press.code.slice(3)
    : press.code.startsWith('Digit') ? press.code.slice(5)
      : press.code
  const parts = [
    press.ctrlKey && 'Ctrl',
    press.altKey && 'Alt',
    press.shiftKey && 'Shift',
    press.metaKey && 'Super',
    name,
  ].filter((part): part is string => Boolean(part))
  return { accelerator: parts.join('+') }
}
