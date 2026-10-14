// @vitest-environment jsdom
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { useAccountUsageDemand } from './use-account-usage'
const original=window.matouDesktop
function Demand({visible}:{visible:boolean}) { useAccountUsageDemand(visible);return null }
afterEach(()=>{cleanup();window.matouDesktop=original;vi.restoreAllMocks()})
it('panel close keeps HUD polling, background stops it, foreground resumes, last consumer releases it',()=>{
  const command=vi.fn(async()=>({}))
  window.matouDesktop={claudeAccountCommand:command} as never
  let focused=true
  vi.spyOn(document,'hasFocus').mockImplementation(()=>focused)
  const hud=render(<Demand visible/>),panel=render(<Demand visible/>)
  panel.unmount()
  expect(command).toHaveBeenLastCalledWith({type:'usage-visibility',visible:true,recheck:true})
  focused=false;act(()=>window.dispatchEvent(new Event('blur')))
  expect(command).toHaveBeenLastCalledWith({type:'usage-visibility',visible:false,recheck:true})
  focused=true;act(()=>window.dispatchEvent(new Event('focus')))
  expect(command).toHaveBeenLastCalledWith({type:'usage-visibility',visible:true,recheck:true})
  hud.unmount();expect(command).toHaveBeenLastCalledWith({type:'usage-visibility',visible:false,recheck:true})
})
