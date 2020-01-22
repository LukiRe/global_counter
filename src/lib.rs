//! This is a minimal library implementing global, thread-safe counters.

extern crate lazy_static;

// We need to pub use lazy_static, as global_(default_)counter! is expanded to a lazy_static! call.
// Absolute paths wont help here.
// TODO: Think of a way to only pub reexport the lazy_static! macro.
#[doc(hidden)]
pub use lazy_static::*;

// Hack for macro export.
// In foreign crates, `global_counter::generic::Counter` will be the name of our counter,
// but in this crate (for testing), we need to artificially introduce this path.
// TODO: Think of a better way to do this.
#[doc(hidden)]
pub mod global_counter {
    pub mod generic {
        pub type Counter<T> = crate::generic::Counter<T>;
    }
}

/// This module contains atomic counters for primitive integer types.
pub mod primitive {
    use std::cell::UnsafeCell;
    use std::sync::atomic::{
        AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16, AtomicU32, AtomicU64,
        AtomicU8, AtomicUsize, Ordering,
    };
    use std::thread::LocalKey;

    /// A flushing counter.
    /// 
    /// This counter is intended to be used in one specific way: 
    /// First, all counting threads increment the counter,
    /// then, every counting thread calls `flush` after it is done incrementing,
    /// then, after every flush is guaranteed to have been executed, `get` will return the exact amount of times `inc` has been called (+ the starting offset).
    /// 
    /// In theory, this counter is equivalent to an approximate counter with its resolution set to infinity.
    pub struct FlushingCounter {
        global_counter: AtomicUsize,

        // This could also be a RefCell, but this impl is also safe- or at least I hope so-
        // and more efficient, as no runtime borrowchecking is needed.
        thread_local_counter: &'static LocalKey<UnsafeCell<usize>>,
    }

    impl FlushingCounter{
        /// Creates a new counter, with the given starting value. Can be used in static contexts.
        #[inline]
        pub const fn new(start: usize) -> Self {
            thread_local!(pub static TL_COUNTER : UnsafeCell<usize> = UnsafeCell::new(0));
            FlushingCounter {
                global_counter: AtomicUsize::new(start),
                thread_local_counter: &TL_COUNTER,
            }
        }

        /// Increments the counter by one.
        #[inline]
        pub fn inc(&self) {
            self.thread_local_counter.with(|tlc| unsafe {
                // This is safe, because concurrent accesses to a thread-local are obviously not possible,
                // and aliasing is not possible using the counters API.
                *tlc.get() += 1;
            });
        }

        /// Gets the current value of the counter. This only returns the correct value after all local counters have been flushed.
        #[inline]
        pub fn get(&self) -> usize {
            self.global_counter.load(Ordering::Relaxed)
        }

        /// Flushes the local counter to the global.
        ///
        /// For more information, see the struct-level documentation.
        #[inline]
        pub fn flush(&self) {
            self.thread_local_counter.with(|tlc| unsafe {
                let tlc = &mut *tlc.get();
                self.global_counter.fetch_add(*tlc, Ordering::Relaxed);
                *tlc = 0;
            });
        }
    }

    /// An approximate counter.
    ///
    /// This counter operates by having a local counter for each thread, which is occasionally flushed to the main global counter.
    ///
    /// The accuracy of the counter is determined by its `resolution` and the number of threads counting on it:
    /// The value returned by `get` is guaranteed to always be less than or to equal this number of threads multiplied with the resolution minus one
    /// away from the actual amount of times `inc` has been called (+ start offset): 
    /// 
    /// `|get - (actual + start)| <= num_threads * (resolution - 1)`
    /// 
    /// 
    /// This is the only guarantee made.
    ///
    /// Setting the resolution to 1 will just make it a worse primitive counter, don't do that. Increasing the resolution increases this counters performance.
    ///
    /// This counter also features a `flush` method,
    /// which can be used to manually flush the local counter of the current thread, increasing the accuracy,
    /// and ultimately making it possible to achieve absolute accuracy.
    ///
    /// This counter is ony available for usize, if you need other types drop by the repo and open an issue.
    /// I wasn't able to think of a reason why somebody would want to approximately count using i8s.
    pub struct ApproxCounter {
        threshold: usize,
        global_counter: AtomicUsize,

        // This could also be a RefCell, but this impl is also safe- or at least I hope so-
        // and more efficient, as no runtime borrowchecking is needed.
        thread_local_counter: &'static LocalKey<UnsafeCell<usize>>,
    }

    impl ApproxCounter {
        // TODO: Evaluate which atomic ordering is the minimum upholding all these guarantees.
        // Proof needed, altough relaxed seems to pass all tests.

        /// Creates a new counter, with the given start value and resolution. Can be used in static contexts.
        #[inline]
        pub const fn new(start: usize, resolution: usize) -> Self {
            thread_local!(pub static TL_COUNTER : UnsafeCell<usize> = UnsafeCell::new(0));
            ApproxCounter {
                threshold: resolution,
                global_counter: AtomicUsize::new(start),
                thread_local_counter: &TL_COUNTER,
            }
        }

        /// Increments the counter by one.
        ///
        /// Note that this call will probably leave the value returned by `get` unchanged.
        #[inline]
        pub fn inc(&self) {
            self.thread_local_counter.with(|tlc| unsafe {
                // This is safe, because concurrent accesses to a thread-local are obviously not possible,
                // and aliasing is not possible using the counters API.
                let tlc = &mut *tlc.get();
                *tlc += 1;
                if *tlc >= self.threshold {
                    self.global_counter.fetch_add(*tlc, Ordering::SeqCst);
                    *tlc = 0;
                }
            });
        }

        /// Gets the current value of the counter. For more information, see the struct-level documentation.
        ///
        /// Especially note, that two calls to `get` with one `inc` interleaved are not guaranteed to, and almost certainely wont, return different values.
        #[inline]
        pub fn get(&self) -> usize {
            self.global_counter.load(Ordering::SeqCst)
        }

        /// Flushes the local counter to the global.
        ///
        /// Note that this only means the local counter of the thread calling is flushed. If you want to flush the local counters of N threads,
        /// each thread needs to call this.
        ///
        /// If every thread which incremented this counter has flushed its local counter, and no other increments have been made or are being made,
        /// a subsequent call to `get` is guaranteed to return the exact count.
        /// However, if you can make use of this, consider if a [FlushingCounter](struct.FlushingCounter.html) fits your usecase better.
        // TODO: Introduce example(s).
        #[inline]
        pub fn flush(&self) {
            self.thread_local_counter.with(|tlc| unsafe {
                let tlc = &mut *tlc.get();
                self.global_counter.fetch_add(*tlc, Ordering::SeqCst);
                *tlc = 0;
            });
        }
    }

    macro_rules! primitive_counter {
        ($( $primitive:ident $atomic:ident $counter:ident ), *) => {
            $(
                /// A primitive counter, implemented using atomics from `std::sync::atomic`.
                ///
                /// This counter makes all the same guarantees a generic counter does.
                /// Especially, calling `inc` N times from different threads will always result in the counter effectively being incremented by N.
                ///
                /// Please note that Atomics may, depending on your compilation target, not be implemented using atomic instructions
                /// (See [here](https://llvm.org/docs/Atomics.html), 'Atomics and Codegen', l.7-11).
                /// Meaning, although lock-freedom is always guaranteed, wait-freedom is not.
                ///
                /// The given atomic ordering is rusts [core::sync::atomic::Ordering](https://doc.rust-lang.org/core/sync/atomic/enum.Ordering.html),
                /// with `AcqRel` translating to `AcqRel`, `Acq` or `Rel`, depending on the operation performed.
                ///
                /// This counter should in general be superior in performance, compared to the equivalent generic counter.
                #[derive(Debug)]
                pub struct $counter($atomic, Ordering);

                impl $counter{
                    /// Creates a new primitive counter. Can be used in const contexts.
                    /// Uses the default `Ordering::SeqCst`, making the strongest ordering guarantees.
                    #[inline]
                    pub const fn new(val : $primitive) -> $counter{
                        $counter($atomic::new(val), Ordering::SeqCst)
                    }

                    /// Creates a new primitive counter with the given atomic ordering. Can be used in const contexts.
                    ///
                    /// Possible orderings are `Relaxed`, `AcqRel` and `SeqCst`.
                    /// Supplying an other ordering is undefined behaviour.
                    #[inline]
                    pub const fn with_ordering(val : $primitive, ordering : Ordering) -> $counter{
                        $counter($atomic::new(val), ordering)
                    }

                    /// Gets the current value of the counter.
                    #[inline]
                    pub fn get(&self) -> $primitive{
                        self.0.load(match self.1{ Ordering::AcqRel => Ordering::Acquire, other => other })
                    }

                    /// Sets the counter to a new value.
                    #[inline]
                    pub fn set(&self, val : $primitive){
                        self.0.store(val, match self.1{ Ordering::AcqRel => Ordering::Release, other => other });
                    }

                    /// Increments the counter by one, returning the previous value.
                    #[inline]
                    pub fn inc(&self) -> $primitive{
                        self.0.fetch_add(1, self.1)
                    }

                    /// Resets the counter to zero.
                    #[inline]
                    pub fn reset(&self){
                        self.0.store(0, match self.1{ Ordering::AcqRel => Ordering::Release, other => other });
                    }
                }
            )*
        };
    }

    primitive_counter![u8 AtomicU8 CounterU8, u16 AtomicU16 CounterU16, u32 AtomicU32 CounterU32, u64 AtomicU64 CounterU64, usize AtomicUsize CounterUsize, i8 AtomicI8 CounterI8, i16 AtomicI16 CounterI16, i32 AtomicI32 CounterI32, i64 AtomicI64 CounterI64, isize AtomicIsize CounterIsize];
}

/// This module contains a generic, thread-safe counter and the accompanying `Inc` trait.
pub mod generic {

    #[cfg(parking_lot)]
    use parking_lot::Mutex;

    #[cfg(not(parking_lot))]
    use std::sync::Mutex;

    /// This trait promises incrementing behaviour.
    /// Implemented for standard integer types.
    /// The current value is mutated, becoming the new, incremented value.
    pub trait Inc {
        fn inc(&mut self);
    }

    macro_rules! imp {
    ($( $t:ty ) *) => {
        $(
            impl Inc for $t{
                #[inline]
                fn inc(&mut self){
                    *self += 1;
                }
            }
        )*
    };
    }

    imp![u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize];

    /// A generic counter.
    ///
    /// This counter is `Send + Sync` regardless of its contents, meaning it is always globally available from all threads, concurrently.
    ///
    /// Implement `Inc` by supplying an impl for incrementing your type. This implementation does not need to be thread-safe.
    #[derive(Debug, Default)]
    pub struct Counter<T: Inc>(Mutex<T>);

    /// Creates a new generic, global counter, starting from the given value.
    ///
    /// # Example
    /// ```
    /// # #[macro_use] use crate::global_counter::*;
    /// type CountedType = u32;
    /// fn main(){
    ///     const start_value : u32 = 0;
    ///     global_counter!(COUNTER_NAME, CountedType, start_value);
    ///     assert_eq!(COUNTER_NAME.get_cloned(), 0);
    ///     COUNTER_NAME.inc();
    ///     assert_eq!(COUNTER_NAME.get_cloned(), 1);
    /// }
    /// ```
    #[macro_export]
    macro_rules! global_counter {
        ($name:ident, $type:ident, $value:expr) => {
            lazy_static! {
                static ref $name: global_counter::generic::Counter<$type> =
                    global_counter::generic::Counter::new($value);
            }
        };
    }

    /// Creates a new generic, global counter, starting from its (inherited) default value.
    ///
    /// This macro will fail compilation if the given type is not `Default`.
    ///
    /// # Example
    /// ```
    /// # #[macro_use] use crate::global_counter::*;
    /// type CountedType = u32;
    /// fn main(){
    ///     global_default_counter!(COUNTER_NAME, CountedType);
    ///     assert_eq!(COUNTER_NAME.get_cloned(), 0);
    ///     COUNTER_NAME.inc();
    ///     assert_eq!(COUNTER_NAME.get_cloned(), 1);
    /// }
    /// ```
    #[macro_export]
    macro_rules! global_default_counter {
        ($name:ident, $type:ty) => {
            lazy_static! {
                static ref $name: global_counter::generic::Counter<$type> =
                    global_counter::generic::Counter::default();
            }
        };
    }

    impl<T: Inc> Counter<T> {
        /// Creates a new generic counter
        ///
        /// This function is not const yet. As soon as [Mutex::new()](https://docs.rs/lock_api/*/lock_api/struct.Mutex.html#method.new) is stable as `const fn`, this will be as well, if the `parking_lot` feature is not disabled.
        /// Then, the exported macros will no longer be needed.
        #[inline]
        pub fn new(val: T) -> Counter<T> {
            Counter(Mutex::new(val))
        }

        /// Returns (basically) an immutable borrow of the underlying value.
        /// Best make sure this borrow goes out of scope before any other methods of the counter are being called.
        ///
        /// If `T` is not `Clone`, this is the only way to access the current value of the counter.
        ///
        /// **Warning**: Attempting to access the counter from the thread holding this borrow will result in a deadlock or panic.
        /// As long as this borrow is alive, no accesses to the counter from any thread are possible.
        ///
        /// # Good Example - Borrow goes out of scope
        /// ```
        /// # #[macro_use] use crate::global_counter::*;
        /// fn main(){
        ///     global_default_counter!(COUNTER, u8);
        ///     assert_eq!(0, *COUNTER.get_borrowed());
        ///
        ///     // The borrow is already out of scope, we can call inc safely.
        ///     COUNTER.inc();
        ///
        ///     assert_eq!(1, *COUNTER.get_borrowed());}
        /// ```
        ///
        /// # Good Example - At most one concurrent access per thread
        /// ```
        /// # #[macro_use] use crate::global_counter::*;
        /// fn main(){
        ///     global_default_counter!(COUNTER, u8);
        ///     assert_eq!(0, *COUNTER.get_borrowed());
        ///     
        ///     // Using this code, there is no danger of data races, race coditions whatsoever.
        ///     // As at each point in time, each thread either has a borrow of the counters value alive,
        ///     // or is accessing the counter using its api, never both at the same time.
        ///     let t1 = std::thread::spawn(move || {
        ///         COUNTER.inc();
        ///         let value_borrowed = COUNTER.get_borrowed();
        ///         assert!(1 <= *value_borrowed, *value_borrowed <= 3);
        ///     });
        ///     let t2 = std::thread::spawn(move || {
        ///         COUNTER.inc();
        ///         let value_borrowed = COUNTER.get_borrowed();
        ///         assert!(1 <= *value_borrowed, *value_borrowed <= 3);
        ///     });
        ///     let t3 = std::thread::spawn(move || {
        ///         COUNTER.inc();
        ///         let value_borrowed = COUNTER.get_borrowed();
        ///         assert!(1 <= *value_borrowed, *value_borrowed <= 3);
        ///     });
        ///
        ///     t1.join().unwrap();
        ///     t2.join().unwrap();
        ///     t3.join().unwrap();
        ///     
        ///     assert_eq!(3, *COUNTER.get_borrowed());}
        /// ```
        ///
        /// # Bad Example
        /// ```no_run
        /// # #[macro_use] use crate::global_counter::*;
        /// // We spawn a new thread. This thread will try lockig the counter twice, causing a deadlock.
        /// std::thread::spawn(move || {
        ///
        ///     // We could also use get_cloned with this counter, circumventing all these troubles.
        ///     global_default_counter!(COUNTER, u32);
        ///     
        ///     // The borrow is now alive, and this thread now holds a lock onto the counter.
        ///     let counter_value_borrowed = COUNTER.get_borrowed();
        ///     assert_eq!(0, *counter_value_borrowed);
        ///
        ///     // Now we try to lock the counter again, but we already hold a lock in the current thread! Deadlock!
        ///     COUNTER.inc();
        ///     
        ///     // Here we use `counter_value_borrowed` again, ensuring it can't be dropped "fortunately".
        ///     // This line will never actually be reached.
        ///     assert_eq!(0, *counter_value_borrowed);
        /// });
        /// ```
        #[inline]
        pub fn get_borrowed(&self) -> impl std::ops::Deref<Target = T> + '_ {
            self.lock()
        }

        /// Returns a mutable borrow of the counted value, meaning the actual value counted by this counter can be mutated through this borrow.
        ///
        /// The constraints pointed out for [get_borrowed](struct.Counter.html#method.get_borrowed) also apply here.
        ///
        /// Although this API is in theory as safe as its immutable equivalent, usage of it is discouraged, as it is highly unidiomatic.
        #[inline]
        pub fn get_mut_borrowed(&self) -> impl std::ops::DerefMut<Target = T> + '_ {
            self.lock()
        }

        /// Sets the counted value to the given value.
        #[inline]
        pub fn set(&self, val: T) {
            *self.lock() = val;
        }

        /// Increments the counter, delegating the specific implementation to the [Inc](trait.Inc.html) trait.
        #[inline]
        pub fn inc(&self) {
            self.lock().inc();
        }

        #[cfg(parking_lot)]
        #[inline]
        fn lock(&self) -> impl std::ops::DerefMut<Target = T> + '_ {
            self.0.lock()
        }

        #[cfg(not(parking_lot))]
        #[inline]
        fn lock(&self) -> impl std::ops::DerefMut<Target = T> + '_ {
            self.0.lock().unwrap()
        }
    }

    impl<T: Inc + Clone> Counter<T> {
        /// This avoid the troubles of [get_borrowed](struct.Counter.html#method.get_borrowed) by cloning the current value.
        ///
        /// Creating a deadlock using this API should be impossible.
        /// The downside of this approach is the cost of a forced clone which may, depending on your use case, not be affordable.
        #[inline]
        pub fn get_cloned(&self) -> T {
            self.lock().clone()
        }

        /// Increments the counter, returning the previous value, cloned.
        #[inline]
        pub fn inc_cloning(&self) -> T {
            let prev = self.get_cloned();
            self.inc();
            prev
        }
    }

    impl<T: Inc + Default> Counter<T> {
        /// Resets the counter to its default value.
        #[inline]
        pub fn reset(&self) {
            self.set(T::default());
        }
    }
}

// TODO: Think about test organization.
// Maybe a seperate test crate would be better?
// Or a seperate file?
// Should codecov be set up?
// What about Travis? Necessary?

#[cfg(test)]
mod tests {

    #[cfg(test)]
    mod generic {

        #![allow(unused_attributes)]
        #[macro_use]
        use crate::*;

        // TODO: Clean up this mess.
        // Maybe move all test helper structs to an extra module.

        #[derive(Default, PartialEq, Eq, Debug)]
        struct PanicOnClone(i32);

        impl Clone for PanicOnClone {
            fn clone(&self) -> Self {
                panic!("PanicOnClone cloned");
            }
        }

        impl crate::generic::Inc for PanicOnClone {
            fn inc(&mut self) {
                self.0.inc();
            }
        }

        #[test]
        fn get_borrowed_doesnt_clone() {
            global_default_counter!(COUNTER, PanicOnClone);
            assert_eq!(*COUNTER.get_borrowed(), PanicOnClone(0));
        }

        #[test]
        fn get_mut_borrowed_doesnt_clone() {
            global_default_counter!(COUNTER, PanicOnClone);
            assert_eq!(*COUNTER.get_mut_borrowed(), PanicOnClone(0));
        }

        #[test]
        fn count_to_five_single_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 1);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 2);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 3);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 4);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 5);
        }

        // TODO: Clean up this mess

        #[derive(Clone, Default, PartialEq, Eq, Debug)]
        struct Baz<T> {
            i: i32,
            u: i32,
            _marker: std::marker::PhantomData<T>,
        }

        impl<T> crate::generic::Inc for Baz<T> {
            fn inc(&mut self) {
                self.i += 1;
            }
        }

        type Bar = Baz<std::cell::RefCell<u32>>;

        #[test]
        fn count_struct() {
            global_default_counter!(COUNTER, Bar);
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 0,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
            COUNTER.inc();
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 1,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
            COUNTER.inc();
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 2,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
            COUNTER.inc();
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 3,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
            COUNTER.inc();
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 4,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
            COUNTER.inc();
            assert_eq!(
                COUNTER.get_cloned(),
                Baz {
                    i: 5,
                    u: 0,
                    _marker: std::marker::PhantomData
                }
            );
        }

        #[test]
        fn count_to_50000_single_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);

            for _ in 0..50000 {
                COUNTER.inc();
            }

            assert_eq!(COUNTER.get_cloned(), 50000);
        }

        #[test]
        fn count_to_five_seq_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);

            let t_0 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_0.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 1);

            let t_1 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_1.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 2);

            let t_2 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_2.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 3);

            let t_3 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_3.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 4);

            let t_4 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_4.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 5);
        }

        #[test]
        fn count_to_50000_seq_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_0.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 10000);

            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_1.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 20000);

            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_2.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 30000);

            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_3.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 40000);

            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_4.join().expect("Err joining thread");
            assert_eq!(COUNTER.get_cloned(), 50000);
        }

        #[test]
        fn count_to_five_par_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);

            let t_0 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_1 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_2 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_3 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_4 = std::thread::spawn(|| {
                COUNTER.inc();
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert_eq!(COUNTER.get_cloned(), 5);
        }

        #[test]
        fn count_to_50000_par_threaded() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert_eq!(COUNTER.get_cloned(), 50000);
        }

        #[test]
        fn reset() {
            global_default_counter!(COUNTER, u32);
            assert_eq!(COUNTER.get_cloned(), 0);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 1);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 2);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 3);

            COUNTER.reset();
            assert_eq!(COUNTER.get_cloned(), 0);
            COUNTER.inc();
            assert_eq!(COUNTER.get_cloned(), 1);
        }
    }

    #[cfg(test)]
    mod primitive {

        use crate::primitive::*;

        #[test]
        fn approx_new_const() {
            static COUNTER: ApproxCounter = ApproxCounter::new(0, 1024);
            assert_eq!(COUNTER.get(), 0);
            COUNTER.inc();
            assert!(COUNTER.get() <= 1);
        }

        #[test]
        fn approx_flush_single_threaded() {
            static COUNTER: ApproxCounter = ApproxCounter::new(0, 1024);
            assert_eq!(COUNTER.get(), 0);
            COUNTER.inc();
            COUNTER.flush();
            assert_eq!(COUNTER.get(), 1);
        }

        #[test]
        fn approx_count_to_50000_single_threaded() {
            const NUM_THREADS: usize = 1;
            const LOCAL_ACC: usize = 1024;
            const GLOBAL_ACC: usize = LOCAL_ACC * NUM_THREADS;
            static COUNTER: ApproxCounter = ApproxCounter::new(0, LOCAL_ACC);
            assert_eq!(COUNTER.get(), 0);

            for _ in 0..50000 {
                COUNTER.inc();
            }

            assert!(50000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 50000 + GLOBAL_ACC);
        }

        #[test]
        fn approx_count_to_50000_seq_threaded() {
            const NUM_THREADS: usize = 5;
            const LOCAL_ACC: usize = 256;
            const GLOBAL_ACC: usize = (LOCAL_ACC - 1) * NUM_THREADS;
            static COUNTER: ApproxCounter = ApproxCounter::new(0, LOCAL_ACC);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_0.join().expect("Err joining thread");
            assert!(10000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 10000 + GLOBAL_ACC);

            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_1.join().expect("Err joining thread");
            assert!(20000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 20000 + GLOBAL_ACC);

            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_2.join().expect("Err joining thread");
            assert!(30000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 30000 + GLOBAL_ACC);

            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_3.join().expect("Err joining thread");
            assert!(40000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 40000 + GLOBAL_ACC);

            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_4.join().expect("Err joining thread");
            assert!(50000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 50000 + GLOBAL_ACC);
        }

        #[test]
        fn approx_count_to_50000_par_threaded() {
            const NUM_THREADS: usize = 5;
            const LOCAL_ACC: usize = 419;
            const GLOBAL_ACC: usize = (LOCAL_ACC - 1) * NUM_THREADS;
            static COUNTER: ApproxCounter = ApproxCounter::new(0, LOCAL_ACC);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert!(50000 - GLOBAL_ACC <= COUNTER.get() && COUNTER.get() <= 50000 + GLOBAL_ACC);
        }

        #[test]
        fn approx_flushed_count_to_50000_par_threaded() {
            const LOCAL_ACC: usize = 419;
            static COUNTER: ApproxCounter = ApproxCounter::new(0, LOCAL_ACC);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
                COUNTER.flush();
            });
            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
                COUNTER.flush();
            });
            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
                COUNTER.flush();
            });
            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
                COUNTER.flush();
            });
            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
                COUNTER.flush();
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert_eq!(50000, COUNTER.get());
        }

        #[test]
        fn primitive_new_const() {
            static COUNTERU8: CounterU8 = CounterU8::new(0);
            assert_eq!(COUNTERU8.get(), 0);
            COUNTERU8.inc();
            assert_eq!(COUNTERU8.get(), 1);

            static COUNTERU16: CounterU16 = CounterU16::new(0);
            assert_eq!(COUNTERU16.get(), 0);
            COUNTERU16.inc();
            assert_eq!(COUNTERU16.get(), 1);

            static COUNTERU32: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTERU32.get(), 0);
            COUNTERU32.inc();
            assert_eq!(COUNTERU32.get(), 1);

            static COUNTERU64: CounterU64 = CounterU64::new(0);
            assert_eq!(COUNTERU64.get(), 0);
            COUNTERU64.inc();
            assert_eq!(COUNTERU64.get(), 1);

            static COUNTERUSIZE: CounterUsize = CounterUsize::new(0);
            assert_eq!(COUNTERUSIZE.get(), 0);
            COUNTERUSIZE.inc();
            assert_eq!(COUNTERUSIZE.get(), 1);

            static COUNTERI8: CounterI8 = CounterI8::new(0);
            assert_eq!(COUNTERI8.get(), 0);
            COUNTERI8.inc();
            assert_eq!(COUNTERI8.get(), 1);

            static COUNTERI16: CounterI16 = CounterI16::new(0);
            assert_eq!(COUNTERI16.get(), 0);
            COUNTERI16.inc();
            assert_eq!(COUNTERI16.get(), 1);

            static COUNTERI32: CounterI32 = CounterI32::new(0);
            assert_eq!(COUNTERI32.get(), 0);
            COUNTERI32.inc();
            assert_eq!(COUNTERI32.get(), 1);

            static COUNTERI64: CounterI64 = CounterI64::new(0);
            assert_eq!(COUNTERI64.get(), 0);
            COUNTERI64.inc();
            assert_eq!(COUNTERI64.get(), 1);

            static COUNTERISIZE: CounterIsize = CounterIsize::new(0);
            assert_eq!(COUNTERISIZE.get(), 0);
            COUNTERISIZE.inc();
            assert_eq!(COUNTERISIZE.get(), 1);
        }

        // FIXME: Add with_ordering test.

        #[test]
        fn primitive_reset() {
            static COUNTER: CounterU8 = CounterU8::new(0);
            assert_eq!(COUNTER.get(), 0);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 1);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 2);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 3);
            COUNTER.reset();
            assert_eq!(COUNTER.get(), 0);
        }

        #[test]
        fn count_to_five_single_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 1);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 2);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 3);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 4);
            COUNTER.inc();
            assert_eq!(COUNTER.get(), 5);
        }

        #[test]
        fn count_to_50000_single_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);

            for _ in 0..50000 {
                COUNTER.inc();
            }

            assert_eq!(COUNTER.get(), 50000);
        }

        #[test]
        fn count_to_five_seq_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_0.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 1);

            let t_1 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_1.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 2);

            let t_2 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_2.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 3);

            let t_3 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_3.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 4);

            let t_4 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            t_4.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 5);
        }

        #[test]
        fn count_to_50000_seq_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_0.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 10000);

            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_1.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 20000);

            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_2.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 30000);

            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_3.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 40000);

            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            t_4.join().expect("Err joining thread");
            assert_eq!(COUNTER.get(), 50000);
        }

        #[test]
        fn count_to_five_par_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_1 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_2 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_3 = std::thread::spawn(|| {
                COUNTER.inc();
            });
            let t_4 = std::thread::spawn(|| {
                COUNTER.inc();
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert_eq!(COUNTER.get(), 5);
        }

        #[test]
        fn count_to_50000_par_threaded() {
            static COUNTER: CounterU32 = CounterU32::new(0);
            assert_eq!(COUNTER.get(), 0);

            let t_0 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_1 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_2 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_3 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });
            let t_4 = std::thread::spawn(|| {
                for _ in 0..10000 {
                    COUNTER.inc();
                }
            });

            t_0.join().expect("Err joining thread");
            t_1.join().expect("Err joining thread");
            t_2.join().expect("Err joining thread");
            t_3.join().expect("Err joining thread");
            t_4.join().expect("Err joining thread");

            assert_eq!(COUNTER.get(), 50000);
        }
    }
}
