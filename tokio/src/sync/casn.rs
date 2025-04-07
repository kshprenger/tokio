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
        let mut r: usize;

        loop {
            r = (*(*d).a2)
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
                break;
            }
        }

        if r == (*d).o2 {
            Self::complete(d);
        }

        return r;
    }

    unsafe fn complete(d: *const RDCSSDescriptor) {
        let v = (*(*d).a1).load(Ordering::SeqCst);
        #[allow(unused)]
        (*(*d).a2).compare_exchange_weak(
            (d as usize) | RDCSS_DESCRIPTOR_MASK,
            if v == (*d).o1 { (*d).n2 } else { (*d).o2 },
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    fn read(addr: &AtomicUsize) -> usize {
        let mut r: usize;
        loop {
            r = addr.load(Ordering::SeqCst);
            if Self::is_desciptor(r) {
                unsafe {
                    Self::complete((r & PTR_MASK) as *const RDCSSDescriptor);
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

    unsafe fn cas(cd_ptr: *const CASNDescriptor) -> bool {
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
                        (cd_ptr as usize) | CASN_DESCRIPTOR_MASK,
                    );

                    let val = RDCSSDescriptor::cas(&rdcss as *const RDCSSDescriptor);
                    if Self::is_descriptor(val) {
                        if val & PTR_MASK != (cd_ptr as usize) {
                            Self::cas((val as *const CASNDescriptor<'_>).as_ref().unwrap());
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
                (cd_ptr as usize) | CASN_DESCRIPTOR_MASK,
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
    unsafe { CASNDescriptor::cas(&casn_descriptor as *const CASNDescriptor) }
}

pub(crate) fn read(addr: &AtomicUsize) -> usize {
    let mut r: usize;

    loop {
        r = RDCSSDescriptor::read(addr);
        if CASNDescriptor::is_descriptor(r) {
            unsafe {
                CASNDescriptor::cas((r & PTR_MASK) as *const CASNDescriptor<'_>);
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
