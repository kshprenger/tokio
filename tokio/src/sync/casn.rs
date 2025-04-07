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

    fn to_ptr(value: usize) -> *const RDCSSDescriptor {
        (value & PTR_MASK) as *const RDCSSDescriptor
    }

    fn to_usize(ptr: *const RDCSSDescriptor) -> usize {
        (ptr as usize) | RDCSS_DESCRIPTOR_MASK
    }

    unsafe fn cas(d_ptr: *const RDCSSDescriptor) -> usize {
        let mut r: usize;

        let d_ref = d_ptr.as_ref().unwrap();

        loop {
            r = (*d_ref.a2)
                .compare_exchange_weak(
                    d_ref.o2,
                    Self::to_usize(d_ptr),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .unwrap_or_else(|prev| prev);
            if Self::is_desciptor(r) {
                Self::complete(Self::to_ptr(r));
            } else {
                break;
            }
        }

        if r == d_ref.o2 {
            Self::complete(d_ptr);
        }

        return r;
    }

    unsafe fn complete(d_ptr: *const RDCSSDescriptor) {
        let d_ref = d_ptr.as_ref().unwrap();
        let v = (*d_ref.a1).load(Ordering::SeqCst);

        #[allow(unused)]
        (*d_ref.a2).compare_exchange_weak(
            Self::to_usize(d_ptr),
            if v == d_ref.o1 { d_ref.n2 } else { d_ref.o2 },
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    fn read(addr: &AtomicUsize) -> usize {
        let mut r: usize;
        loop {
            r = addr.load(Ordering::SeqCst);
            if Self::is_desciptor(r) {
                // SAFETY: r is discriptor pointer stored previously
                unsafe {
                    Self::complete(Self::to_ptr(r));
                }
            } else {
                return r;
            }
        }
    }
}

#[derive(PartialEq)]
enum Status {
    Undecided,
    Succeeded,
    Failed,
}

pub(crate) struct CASNDescriptor<'a> {
    addrs: &'a [&'a AtomicUsize],
    expected: &'a [usize],
    new: &'a [usize],
    status: AtomicUsize,
}

impl<'a> CASNDescriptor<'a> {
    fn new(addrs: &'a [&'a AtomicUsize], expected: &'a [usize], new: &'a [usize]) -> Self {
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

    fn to_usize(ptr: *const CASNDescriptor<'_>) -> usize {
        (ptr as usize) | CASN_DESCRIPTOR_MASK
    }

    fn to_ptr(value: usize) -> *const CASNDescriptor<'a> {
        (value & PTR_MASK) as *const CASNDescriptor<'a>
    }

    unsafe fn cas(cd_ptr: *const CASNDescriptor<'_>) -> bool {
        let cd = cd_ptr.as_ref().unwrap();
        if cd.status.load(Ordering::SeqCst) == Status::Undecided as usize {
            let mut status = Status::Succeeded;
            for i in 0..cd.addrs.len() {
                if status != Status::Succeeded {
                    break;
                }
                loop {
                    let rdcss = RDCSSDescriptor::new(
                        &cd.status as *const AtomicUsize,
                        Status::Undecided as usize,
                        cd.addrs[i],
                        cd.expected[i],
                        Self::to_usize(cd_ptr),
                    );

                    let val = RDCSSDescriptor::cas(&rdcss as *const RDCSSDescriptor);
                    if Self::is_descriptor(val) {
                        if val & PTR_MASK != (cd_ptr as usize) {
                            Self::cas(Self::to_ptr(val));
                            continue;
                        }
                    } else if val != cd.expected[i] {
                        status = Status::Failed;
                    }
                    break;
                }
            }

            #[allow(unused)]
            cd.status.compare_exchange_weak(
                Status::Undecided as usize,
                status as usize,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }

        let succeeded = cd.status.load(Ordering::SeqCst) == (Status::Succeeded as usize);
        for i in 0..cd.addrs.len() {
            #[allow(unused)]
            (*cd.addrs[i]).compare_exchange_weak(
                Self::to_usize(cd_ptr),
                if succeeded { cd.new[i] } else { cd.expected[i] },
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        succeeded
    }
}

pub(crate) fn casn(addrs: &[&AtomicUsize], expected: &[usize], new: &[usize]) -> bool {
    assert_eq!(addrs.len(), expected.len());
    assert_eq!(addrs.len(), new.len());
    let casn_descriptor = CASNDescriptor::new(addrs, expected, new);
    // SAFETY: invoking on a valid pointer
    unsafe { CASNDescriptor::cas(&casn_descriptor as *const CASNDescriptor<'_>) }
}

pub(crate) fn read(addr: &AtomicUsize) -> usize {
    let mut r: usize;

    loop {
        r = RDCSSDescriptor::read(addr);
        if CASNDescriptor::is_descriptor(r) {
            // SAFETY: r is a valid casn desciptor stored previously
            unsafe {
                CASNDescriptor::cas(CASNDescriptor::to_ptr(r));
            }
        } else {
            return r;
        }
    }
}

#[cfg(not(loom))]
#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn just_work() {
        let a = AtomicUsize::new(69);
        let b = AtomicUsize::new(1488);
        let memory = &[&a, &b];

        assert_eq!(read(&a), 69);
        assert_eq!(read(&b), 1488);

        casn(memory, &[69, 1488], &[1488, 69]);

        assert_eq!(read(&a), 1488);
        assert_eq!(read(&b), 69);
    }
}
