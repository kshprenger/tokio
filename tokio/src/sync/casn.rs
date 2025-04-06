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
    pub(crate) fn new(
        addrs: &'a [&'a AtomicUsize],
        expected: &'a [usize],
        new: &'a [usize],
    ) -> Self {
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

    fn as_usize(&self) -> usize {
        self as *const CASNDescriptor<'_> as usize
    }

    pub(crate) unsafe fn cas(&self) -> bool {
        if self.status.load(Ordering::SeqCst) == Status::Undecided as usize {
            let mut status = Status::Succeeded;
            for i in 0..self.addrs.len() {
                if status != Status::Succeeded {
                    break;
                }
                loop {
                    let rdcss = RDCSSDescriptor::new(
                        &self.status as *const AtomicUsize,
                        Status::Undecided as usize,
                        self.addrs[i],
                        self.expected[i],
                        self.as_usize() | CASN_DESCRIPTOR_MASK,
                    );

                    let val = RDCSSDescriptor::cas(&rdcss as *const RDCSSDescriptor);
                    if Self::is_descriptor(val) {
                        if val & PTR_MASK != self.as_usize() {
                            Self::cas((val as *const CASNDescriptor<'_>).as_ref().unwrap());
                            continue;
                        }
                    } else if val != self.expected[i] {
                        status = Status::Failed;
                    }
                    break;
                }
            }

            #[allow(unused)]
            self.status.compare_exchange_weak(
                Status::Undecided as usize,
                status as usize,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }

        let succeeded = self.status.load(Ordering::SeqCst) == (Status::Succeeded as usize);
        for i in 0..self.addrs.len() {
            #[allow(unused)]
            (*self.addrs[i]).compare_exchange_weak(
                self.as_usize() | CASN_DESCRIPTOR_MASK,
                if succeeded {
                    self.new[i]
                } else {
                    self.expected[i]
                },
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        succeeded
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

        let casn = CASNDescriptor::new(memory, &[69, 1488], &[1488, 69]);
        unsafe {
            casn.cas();
        }
        assert_eq!(a.load(Ordering::SeqCst), 1488);
        assert_eq!(b.load(Ordering::SeqCst), 69);
    }
}
