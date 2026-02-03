use core_affinity::CoreId;

pub(crate) fn cpu_has_core_else_panic(id: usize) {
    let has_core = core_affinity::get_core_ids()
        .map(|ids| ids.iter().any(|core_id| core_id.id == id))
        .unwrap_or(false);

    if !has_core {
        panic!("No core with ID={} is available.", id);
    }
}

pub(crate) fn set_affinity_if_defined(core_affinity: Option<CoreId>, thread_name: &str) {
    if let Some(core_id) = core_affinity {
        let got_pinned = core_affinity::set_for_current(core_id);
        if !got_pinned {
            eprintln!(
                "Could not pin processor thread '{}' to {:?}.",
                thread_name, core_id
            );
        }
    }
}
