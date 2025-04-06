use crate::loom::sync::atomic::{AtomicUsize, Ordering};

// Platform bit hacks
const CASN_DESCRIPTOR_MASK: usize = 0x8000_0000_0000_0000;
const RDCSS_DESCRIPTOR_MASK: usize = 0x4000_0000_0000_0000;
const PTR_MASK: usize = 0x3FFF_FFFF_FFFF_FFFF;

struct RDCSSDescriptor {
    a1: *const AtomicUsize,
    o1: usize,
    a2: *const AtomicUsize,
    o2: usize,
    n2: usize,
}

impl RDCSSDescriptor {
    fn new(
        a1: *const AtomicUsize,
        o1: usize,
        a2: *const AtomicUsize,
        o2: usize,
        n2: usize,
    ) -> Self {
        RDCSSDescriptor { a1, o1, a2, o2, n2 }
    }

    fn is_desciptor(value: usize) -> bool {
        value & RDCSS_DESCRIPTOR_MASK == RDCSS_DESCRIPTOR_MASK
    }

    unsafe fn cas(d: *const RDCSSDescriptor) -> usize {
        loop {
            let r = (*(*d).a2)
                .compare_exchange_weak(
                    (*d).o2,
                    (d as usize) | RDCSS_DESCRIPTOR_MASK,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .unwrap_or_else(|prev| prev);
            if Self::is_desciptor(r) {
                Self::complete((r & PTR_MASK) as *const RDCSSDescriptor);
            } else {
                return r;
            }
        }
    }

    unsafe fn complete(d: *const RDCSSDescriptor) {
        let v = (*(*d).a1).load(Ordering::SeqCst);
        if v == (*d).o1 {
            #[allow(unused)]
            (*(*d).a2).compare_exchange_weak(
                (d as usize) | RDCSS_DESCRIPTOR_MASK,
                (*d).n2,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        } else {
            #[allow(unused)]
            (*(*d).a2).compare_exchange_weak(
                (d as usize) | RDCSS_DESCRIPTOR_MASK,
                (*d).o2,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }
}

#[derive(PartialEq)]
enum Status {
    Undecided,
    Succeeded,
    Failed,
}

struct CASNDescriptor {
    addrs: Vec<*const AtomicUsize>,
    expected: Vec<usize>,
    new: Vec<usize>,
    status: AtomicUsize,
}

impl CASNDescriptor {
    fn new(addrs: Vec<*const AtomicUsize>, expected: Vec<usize>, new: Vec<usize>) -> Self {
        assert_eq!(addrs.len(), expected.len());
        assert_eq!(addrs.len(), new.len());

        CASNDescriptor {
            addrs,
            expected,
            new,
            status: AtomicUsize::new(Status::Undecided as usize),
        }
    }

    fn is_descriptor(value: usize) -> bool {
        value & CASN_DESCRIPTOR_MASK == CASN_DESCRIPTOR_MASK
    }

    unsafe fn cas(cd: *const CASNDescriptor) -> bool {
        if (*cd).status.load(Ordering::SeqCst) == Status::Undecided as usize {
            let mut status = Status::Succeeded;
            for i in (0..(*cd).addrs.len()).take_while(|_| status == Status::Succeeded) {
                loop {
                    let rdcss = RDCSSDescriptor::new(
                        &(*cd).status as *const AtomicUsize,
                        Status::Undecided as usize,
                        (*cd).addrs[i],
                        (*cd).expected[i],
                        (cd as usize) | CASN_DESCRIPTOR_MASK,
                    );

                    let val = RDCSSDescriptor::cas(&rdcss as *const RDCSSDescriptor);
                    if Self::is_descriptor(val) {
                        if val & PTR_MASK != (cd as usize) {
                            Self::cas(val as *const CASNDescriptor);
                            continue;
                        } else if val != (*cd).expected[i] {
                            status = Status::Failed;
                        }
                    }
                }
            }

            #[allow(unused)]
            (*cd).status.compare_exchange_weak(
                Status::Undecided as usize,
                status as usize,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }

        let succeeded = (*cd).status.load(Ordering::SeqCst) == (Status::Succeeded as usize);
        for i in 0..(*cd).addrs.len() {
            #[allow(unused)]
            (*(*cd).addrs[i]).compare_exchange_weak(
                (cd as usize) | CASN_DESCRIPTOR_MASK,
                if succeeded {
                    (*cd).new[i]
                } else {
                    (*cd).expected[i]
                },
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        succeeded
    }
}
