import { describe, expect, it } from 'vitest'
import { hotkeyFromKey } from './hotkey'

const key = (code: string, mods: Partial<{ ctrlKey: boolean, altKey: boolean, shiftKey: boolean, metaKey: boolean }> = {}, name = code) =>
  ({ code, key: name, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods })

describe('hotkey capture', () => {
  it('names a function key on its own', () => {
    expect(hotkeyFromKey(key('F8'))).toEqual({ accelerator: 'F8' })
  })

  it('names modifiers in a fixed order and letters by their key', () => {
    expect(hotkeyFromKey(key('KeyP', { shiftKey: true, ctrlKey: true }, 'P'))).toEqual({ accelerator: 'Ctrl+Shift+P' })
    expect(hotkeyFromKey(key('Digit7', { altKey: true }, '7'))).toEqual({ accelerator: 'Alt+7' })
    expect(hotkeyFromKey(key('Numpad5', { metaKey: true }, '5'))).toEqual({ accelerator: 'Super+Numpad5' })
  })

  it('keeps waiting while only a modifier is down', () => {
    expect(hotkeyFromKey(key('ControlLeft', { ctrlKey: true }, 'Control'))).toBeNull()
    expect(hotkeyFromKey(key('ShiftRight', { shiftKey: true }, 'Shift'))).toBeNull()
  })

  it('refuses a bare typing key, which would swallow it in game chat', () => {
    expect(hotkeyFromKey(key('KeyP', {}, 'p'))).toEqual({ problem: 'Add Ctrl or Alt to a letter, number or Space.' })
    expect(hotkeyFromKey(key('Space', {}, ' '))).toEqual({ problem: 'Add Ctrl or Alt to a letter, number or Space.' })
    // Shift alone still types a capital in chat.
    expect(hotkeyFromKey(key('KeyP', { shiftKey: true }, 'P'))).toEqual({ problem: 'Add Ctrl or Alt to a letter, number or Space.' })
  })
})
