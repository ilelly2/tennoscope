import { invoke } from '@tauri-apps/api/core'

export async function showRewardOverlay() {
  await invoke('show_reward_overlay')
}

export async function hideRewardOverlay() {
  await invoke('hide_reward_overlay')
}

/** Whether the reward strip is up right now. The toggle needs the truth when it mounts. */
export async function getRewardOverlayVisibility() {
  return await invoke<boolean>('get_reward_overlay_visibility')
}
