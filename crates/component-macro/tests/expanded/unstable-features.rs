pub enum Baz {}
pub trait HostBaz {
    fn foo(&mut self, self_: wasmtime::component::Resource<Baz>) -> ();
    fn drop(&mut self, rep: wasmtime::component::Resource<Baz>) -> wasmtime::Result<()>;
}
impl<_T: HostBaz + ?Sized> HostBaz for &mut _T {
    fn foo(&mut self, self_: wasmtime::component::Resource<Baz>) -> () {
        HostBaz::foo(*self, self_)
    }
    fn drop(&mut self, rep: wasmtime::component::Resource<Baz>) -> wasmtime::Result<()> {
        HostBaz::drop(*self, rep)
    }
}
/// Auto-generated bindings for a pre-instantiated version of a
/// component which implements the world `the-world`.
///
/// This structure is created through [`TheWorldPre::new`] which
/// takes a [`InstancePre`](wasmtime::component::InstancePre) that
/// has been created through a [`Linker`](wasmtime::component::Linker).
///
/// For more information see [`TheWorld`] as well.
pub struct TheWorldPre<T> {
    instance_pre: wasmtime::component::InstancePre<T>,
    indices: TheWorldIndices,
}
impl<T> Clone for TheWorldPre<T> {
    fn clone(&self) -> Self {
        Self {
            instance_pre: self.instance_pre.clone(),
            indices: self.indices.clone(),
        }
    }
}
impl<_T> TheWorldPre<_T> {
    /// Creates a new copy of `TheWorldPre` bindings which can then
    /// be used to instantiate into a particular store.
    ///
    /// This method may fail if the component behind `instance_pre`
    /// does not have the required exports.
    pub fn new(
        instance_pre: wasmtime::component::InstancePre<_T>,
    ) -> wasmtime::Result<Self> {
        let indices = TheWorldIndices::new(instance_pre.component())?;
        Ok(Self { instance_pre, indices })
    }
    pub fn engine(&self) -> &wasmtime::Engine {
        self.instance_pre.engine()
    }
    pub fn instance_pre(&self) -> &wasmtime::component::InstancePre<_T> {
        &self.instance_pre
    }
    /// Instantiates a new instance of [`TheWorld`] within the
    /// `store` provided.
    ///
    /// This function will use `self` as the pre-instantiated
    /// instance to perform instantiation. Afterwards the preloaded
    /// indices in `self` are used to lookup all exports on the
    /// resulting instance.
    pub fn instantiate(
        &self,
        mut store: impl wasmtime::AsContextMut<Data = _T>,
    ) -> wasmtime::Result<TheWorld> {
        let mut store = store.as_context_mut();
        let instance = self.instance_pre.instantiate(&mut store)?;
        self.indices.load(&mut store, &instance)
    }
}
/// Auto-generated bindings for index of the exports of
/// `the-world`.
///
/// This is an implementation detail of [`TheWorldPre`] and can
/// be constructed if needed as well.
///
/// For more information see [`TheWorld`] as well.
#[derive(Clone)]
pub struct TheWorldIndices {}
/// Auto-generated bindings for an instance a component which
/// implements the world `the-world`.
///
/// This structure can be created through a number of means
/// depending on your requirements and what you have on hand:
///
/// * The most convenient way is to use
///   [`TheWorld::instantiate`] which only needs a
///   [`Store`], [`Component`], and [`Linker`].
///
/// * Alternatively you can create a [`TheWorldPre`] ahead of
///   time with a [`Component`] to front-load string lookups
///   of exports once instead of per-instantiation. This
///   method then uses [`TheWorldPre::instantiate`] to
///   create a [`TheWorld`].
///
/// * If you've instantiated the instance yourself already
///   then you can use [`TheWorld::new`].
///
/// * You can also access the guts of instantiation through
///   [`TheWorldIndices::new_instance`] followed
///   by [`TheWorldIndices::load`] to crate an instance of this
///   type.
///
/// These methods are all equivalent to one another and move
/// around the tradeoff of what work is performed when.
///
/// [`Store`]: wasmtime::Store
/// [`Component`]: wasmtime::component::Component
/// [`Linker`]: wasmtime::component::Linker
pub struct TheWorld {}
pub trait TheWorldImports: HostBaz {
    fn foo(&mut self) -> ();
}
pub trait TheWorldImportsGetHost<
    T,
>: Fn(T) -> <Self as TheWorldImportsGetHost<T>>::Host + Send + Sync + Copy + 'static {
    type Host: TheWorldImports;
}
impl<F, T, O> TheWorldImportsGetHost<T> for F
where
    F: Fn(T) -> O + Send + Sync + Copy + 'static,
    O: TheWorldImports,
{
    type Host = O;
}
impl<_T: TheWorldImports + ?Sized> TheWorldImports for &mut _T {
    fn foo(&mut self) -> () {
        TheWorldImports::foo(*self)
    }
}
const _: () = {
    #[allow(unused_imports)]
    use wasmtime::component::__internal::anyhow;
    impl TheWorldIndices {
        /// Creates a new copy of `TheWorldIndices` bindings which can then
        /// be used to instantiate into a particular store.
        ///
        /// This method may fail if the component does not have the
        /// required exports.
        pub fn new(
            component: &wasmtime::component::Component,
        ) -> wasmtime::Result<Self> {
            let _component = component;
            Ok(TheWorldIndices {})
        }
        /// Creates a new instance of [`TheWorldIndices`] from an
        /// instantiated component.
        ///
        /// This method of creating a [`TheWorld`] will perform string
        /// lookups for all exports when this method is called. This
        /// will only succeed if the provided instance matches the
        /// requirements of [`TheWorld`].
        pub fn new_instance(
            mut store: impl wasmtime::AsContextMut,
            instance: &wasmtime::component::Instance,
        ) -> wasmtime::Result<Self> {
            let _instance = instance;
            Ok(TheWorldIndices {})
        }
        /// Uses the indices stored in `self` to load an instance
        /// of [`TheWorld`] from the instance provided.
        ///
        /// Note that at this time this method will additionally
        /// perform type-checks of all exports.
        pub fn load(
            &self,
            mut store: impl wasmtime::AsContextMut,
            instance: &wasmtime::component::Instance,
        ) -> wasmtime::Result<TheWorld> {
            let _instance = instance;
            Ok(TheWorld {})
        }
    }
    impl TheWorld {
        /// Convenience wrapper around [`TheWorldPre::new`] and
        /// [`TheWorldPre::instantiate`].
        pub fn instantiate<_T>(
            mut store: impl wasmtime::AsContextMut<Data = _T>,
            component: &wasmtime::component::Component,
            linker: &wasmtime::component::Linker<_T>,
        ) -> wasmtime::Result<TheWorld> {
            let pre = linker.instantiate_pre(component)?;
            TheWorldPre::new(pre)?.instantiate(store)
        }
        /// Convenience wrapper around [`TheWorldIndices::new_instance`] and
        /// [`TheWorldIndices::load`].
        pub fn new(
            mut store: impl wasmtime::AsContextMut,
            instance: &wasmtime::component::Instance,
        ) -> wasmtime::Result<TheWorld> {
            let indices = TheWorldIndices::new_instance(&mut store, instance)?;
            indices.load(store, instance)
        }
        pub fn add_to_linker_imports_get_host<T>(
            linker: &mut wasmtime::component::Linker<T>,
            host_getter: impl for<'a> TheWorldImportsGetHost<&'a mut T>,
        ) -> wasmtime::Result<()> {
            let mut linker = linker.root();
            linker
                .resource(
                    "baz",
                    wasmtime::component::ResourceType::host::<Baz>(),
                    move |mut store, rep| -> wasmtime::Result<()> {
                        HostBaz::drop(
                            &mut host_getter(store.data_mut()),
                            wasmtime::component::Resource::new_own(rep),
                        )
                    },
                )?;
            linker
                .func_wrap(
                    "foo",
                    move |mut caller: wasmtime::StoreContextMut<'_, T>, (): ()| {
                        let host = &mut host_getter(caller.data_mut());
                        let r = TheWorldImports::foo(host);
                        Ok(r)
                    },
                )?;
            linker
                .func_wrap(
                    "[method]baz.foo",
                    move |
                        mut caller: wasmtime::StoreContextMut<'_, T>,
                        (arg0,): (wasmtime::component::Resource<Baz>,)|
                    {
                        let host = &mut host_getter(caller.data_mut());
                        let r = HostBaz::foo(host, arg0);
                        Ok(r)
                    },
                )?;
            Ok(())
        }
        pub fn add_to_linker<T, U>(
            linker: &mut wasmtime::component::Linker<T>,
            get: impl Fn(&mut T) -> &mut U + Send + Sync + Copy + 'static,
        ) -> wasmtime::Result<()>
        where
            U: foo::foo::the_interface::Host + TheWorldImports,
        {
            Self::add_to_linker_imports_get_host(linker, get)?;
            foo::foo::the_interface::add_to_linker(linker, get)?;
            Ok(())
        }
    }
};
pub mod foo {
    pub mod foo {
        #[allow(clippy::all)]
        pub mod the_interface {
            #[allow(unused_imports)]
            use wasmtime::component::__internal::anyhow;
            pub enum Bar {}
            pub trait HostBar {
                fn foo(&mut self, self_: wasmtime::component::Resource<Bar>) -> ();
                fn drop(
                    &mut self,
                    rep: wasmtime::component::Resource<Bar>,
                ) -> wasmtime::Result<()>;
            }
            impl<_T: HostBar + ?Sized> HostBar for &mut _T {
                fn foo(&mut self, self_: wasmtime::component::Resource<Bar>) -> () {
                    HostBar::foo(*self, self_)
                }
                fn drop(
                    &mut self,
                    rep: wasmtime::component::Resource<Bar>,
                ) -> wasmtime::Result<()> {
                    HostBar::drop(*self, rep)
                }
            }
            pub trait Host: HostBar {
                fn foo(&mut self) -> ();
            }
            pub trait GetHost<
                T,
            >: Fn(T) -> <Self as GetHost<T>>::Host + Send + Sync + Copy + 'static {
                type Host: Host;
            }
            impl<F, T, O> GetHost<T> for F
            where
                F: Fn(T) -> O + Send + Sync + Copy + 'static,
                O: Host,
            {
                type Host = O;
            }
            pub fn add_to_linker_get_host<T>(
                linker: &mut wasmtime::component::Linker<T>,
                host_getter: impl for<'a> GetHost<&'a mut T>,
            ) -> wasmtime::Result<()> {
                let mut inst = linker.instance("foo:foo/the-interface")?;
                inst.resource(
                    "bar",
                    wasmtime::component::ResourceType::host::<Bar>(),
                    move |mut store, rep| -> wasmtime::Result<()> {
                        HostBar::drop(
                            &mut host_getter(store.data_mut()),
                            wasmtime::component::Resource::new_own(rep),
                        )
                    },
                )?;
                inst.func_wrap(
                    "foo",
                    move |mut caller: wasmtime::StoreContextMut<'_, T>, (): ()| {
                        let host = &mut host_getter(caller.data_mut());
                        let r = Host::foo(host);
                        Ok(r)
                    },
                )?;
                inst.func_wrap(
                    "[method]bar.foo",
                    move |
                        mut caller: wasmtime::StoreContextMut<'_, T>,
                        (arg0,): (wasmtime::component::Resource<Bar>,)|
                    {
                        let host = &mut host_getter(caller.data_mut());
                        let r = HostBar::foo(host, arg0);
                        Ok(r)
                    },
                )?;
                Ok(())
            }
            pub fn add_to_linker<T, U>(
                linker: &mut wasmtime::component::Linker<T>,
                get: impl Fn(&mut T) -> &mut U + Send + Sync + Copy + 'static,
            ) -> wasmtime::Result<()>
            where
                U: Host,
            {
                add_to_linker_get_host(linker, get)
            }
            impl<_T: Host + ?Sized> Host for &mut _T {
                fn foo(&mut self) -> () {
                    Host::foo(*self)
                }
            }
        }
    }
}
