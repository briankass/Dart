//! Store / UWP apps, read from the shell's AppsFolder (the same list the
//! Start menu's "All apps" shows). Desktop apps are already covered by the
//! Start Menu scan in `index`, so only packaged apps (AUMIDs with '!') are kept.

use crate::search::Entry;
use windows::core::PWSTR;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::*;

pub fn scan() -> Vec<Entry> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let out = scan_inner().unwrap_or_default();
        if init.is_ok() {
            CoUninitialize();
        }
        out
    }
}

unsafe fn take(p: windows::core::Result<PWSTR>) -> Option<String> {
    let p = p.ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

unsafe fn scan_inner() -> windows::core::Result<Vec<Entry>> {
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let items: IEnumShellItems = folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems)?;
    let mut out = Vec::new();
    loop {
        let mut batch: [Option<IShellItem>; 1] = [None];
        let mut fetched = 0u32;
        if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = batch[0].take() else { break };
        let Some(aumid) = take(item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING)) else { continue };
        if !aumid.contains('!') {
            continue;
        }
        let Some(name) = take(item.GetDisplayName(SIGDN_NORMALDISPLAY)) else { continue };
        out.push(Entry { name, path: format!("shell:AppsFolder\\{aumid}") });
    }
    Ok(out)
}
