use std::{ops::Deref, rc::Rc};

use generational_box::{GenerationalBox, Owner};

pub(crate) struct StateOwner {
    values: Owner,
    changed: GenerationalBox<Rc<dyn Fn()>>,
}

impl StateOwner {
    pub fn new(changed: Rc<dyn Fn()>) -> Self {
        let values = Owner::default();
        let changed = values.insert(changed);
        Self { values, changed }
    }

    pub fn field<T: 'static>(&self, value: T) -> Signal<T> {
        Signal {
            value: self.values.insert(value),
            changed: self.changed,
        }
    }
}

pub struct Signal<T: 'static> {
    value: GenerationalBox<T>,
    changed: GenerationalBox<Rc<dyn Fn()>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Signal<T> {}

impl<T> PartialEq for Signal<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value.ptr_eq(&other.value)
    }
}

pub trait ReadableExt<T> {
    fn peek(&self) -> impl Deref<Target = T> + 'static;
}

impl<T: 'static> ReadableExt<T> for Signal<T> {
    fn peek(&self) -> impl Deref<Target = T> + 'static {
        self.value.read()
    }
}

pub trait WritableExt<T> {
    fn set(&mut self, value: T);
}

impl<T: 'static> WritableExt<T> for Signal<T> {
    fn set(&mut self, value: T) {
        self.value.set(value);
        (self.changed.read())();
    }
}

impl std::ops::AddAssign<u64> for Signal<u64> {
    fn add_assign(&mut self, increment: u64) {
        *self.value.write() += increment;
        (self.changed.read())();
    }
}
