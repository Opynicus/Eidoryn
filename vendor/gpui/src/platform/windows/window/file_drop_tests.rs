use super::paths_from_file_drop;
use std::{
    ffi::c_void,
    mem::{ManuallyDrop, size_of},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, GlobalFree, HGLOBAL, S_OK},
        System::{
            Com::{
                DVASPECT_CONTENT, FORMATETC, IDataObject, ISequentialStream,
                ISequentialStream_Impl, STGMEDIUM, STGMEDIUM_0,
                StructuredStorage::CreateStreamOnHGlobal, TYMED_HGLOBAL, TYMED_ISTREAM,
            },
            Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{CF_HDROP, OleInitialize, OleUninitialize, ReleaseStgMedium},
        },
        UI::Shell::{DROPFILES, SHCreateDataObject},
    },
    core::{HRESULT, Interface, implement},
};

struct OleApartment;

impl OleApartment {
    fn new() -> Self {
        unsafe { OleInitialize(None) }.expect("initialize OLE for the native file-drop test");
        Self
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

fn file_drop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    }
}

fn file_drop_medium(paths: &[&str]) -> STGMEDIUM {
    let mut names: Vec<u16> = paths
        .iter()
        .flat_map(|path| path.encode_utf16().chain(std::iter::once(0)))
        .collect();
    names.push(0);
    if paths.is_empty() {
        names.push(0);
    }

    let header_size = size_of::<DROPFILES>();
    let global = unsafe {
        GlobalAlloc(
            GMEM_MOVEABLE | GMEM_ZEROINIT,
            header_size + names.len() * size_of::<u16>(),
        )
    }
    .expect("allocate a movable CF_HDROP storage medium");
    let data = unsafe { GlobalLock(global) }.cast::<u8>();
    assert!(!data.is_null(), "lock the CF_HDROP storage medium");
    unsafe {
        data.cast::<DROPFILES>().write(DROPFILES {
            pFiles: header_size as u32,
            fWide: true.into(),
            ..Default::default()
        });
        std::ptr::copy_nonoverlapping(
            names.as_ptr(),
            data.add(header_size).cast::<u16>(),
            names.len(),
        );
        // A zero return also means that the final lock was successfully released.
        let _ = GlobalUnlock(global);
    }
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        ..Default::default()
    }
}

fn shell_file_drop(paths: &[&str]) -> IDataObject {
    let data_object: IDataObject = unsafe { SHCreateDataObject(None, None, None) }
        .expect("create the Shell's native IDataObject");
    let mut medium = file_drop_medium(paths);
    if let Err(error) = unsafe { data_object.SetData(&file_drop_format(), &medium, true) } {
        unsafe { ReleaseStgMedium(&mut medium) };
        panic!("populate CF_HDROP on the Shell data object: {error}");
    }
    // SetData(TRUE) transferred ownership to the Shell data object.
    data_object
}

#[test]
fn native_file_drop_reads_unicode_and_multiple_files() {
    let _apartment = OleApartment::new();
    let expected = [
        r"C:\游戏\BIOS\固件.bin",
        r"C:\PS1 Games\Crash Bandicoot (USA).cue",
    ];
    let data_object = shell_file_drop(&expected);
    assert_eq!(
        unsafe { data_object.QueryGetData(&file_drop_format()) },
        S_OK
    );
    let medium = unsafe { data_object.GetData(&file_drop_format()) }
        .expect("retrieve the same CF_HDROP medium used by DragEnter");

    let actual = paths_from_file_drop(medium);

    assert_eq!(actual.as_slice(), expected.map(PathBuf::from).as_slice(),);
}

#[test]
fn native_file_drop_reads_empty_file_list() {
    let _apartment = OleApartment::new();
    let data_object = shell_file_drop(&[]);
    let medium = unsafe { data_object.GetData(&file_drop_format()) }
        .expect("retrieve an empty CF_HDROP file list");

    assert!(paths_from_file_drop(medium).is_empty());
}

// ReleaseStgMedium must honor pUnkForRelease even when the medium is rejected.
// The stream interface provides an ordinary COM IUnknown with observable lifetime;
// the file-drop parser never calls its Read or Write methods.
#[implement(ISequentialStream)]
struct ReleaseOwner {
    releases: Arc<AtomicUsize>,
    global: Option<HGLOBAL>,
}

#[allow(non_snake_case)]
impl ISequentialStream_Impl for ReleaseOwner_Impl {
    fn Read(&self, _pv: *mut c_void, _cb: u32, _pcbread: *mut u32) -> HRESULT {
        E_NOTIMPL
    }

    fn Write(&self, _pv: *const c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        E_NOTIMPL
    }
}

impl Drop for ReleaseOwner {
    fn drop(&mut self) {
        if let Some(global) = self.global {
            // GlobalFree returns NULL on success; windows 0.61 maps that to Err.
            let _ = unsafe { GlobalFree(Some(global)) };
        }
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}

fn add_release_owner(medium: &mut STGMEDIUM) -> Arc<AtomicUsize> {
    let releases = Arc::new(AtomicUsize::new(0));
    let global = unsafe { medium.u.hGlobal };
    let owner: ISequentialStream = ReleaseOwner {
        releases: releases.clone(),
        global: (!global.is_invalid()).then_some(global),
    }
    .into();
    medium.pUnkForRelease = ManuallyDrop::new(Some(owner.cast().unwrap()));
    releases
}

#[test]
fn native_file_drop_releases_medium_after_reading() {
    let mut medium = file_drop_medium(&[r"C:\BIOS\scph1001.bin"]);
    let releases = add_release_owner(&mut medium);

    let paths = paths_from_file_drop(medium);

    assert_eq!(paths.as_slice(), &[PathBuf::from(r"C:\BIOS\scph1001.bin")]);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn native_file_drop_releases_owner_when_handle_is_null() {
    let mut medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    };
    let releases = add_release_owner(&mut medium);

    assert!(paths_from_file_drop(medium).is_empty());
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn native_file_drop_rejects_stream_medium() {
    let _apartment = OleApartment::new();
    let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }
        .expect("create a valid medium with a different storage type");
    let medium = STGMEDIUM {
        tymed: TYMED_ISTREAM.0 as u32,
        u: STGMEDIUM_0 {
            pstm: ManuallyDrop::new(Some(stream)),
        },
        ..Default::default()
    };

    assert!(paths_from_file_drop(medium).is_empty());
}
