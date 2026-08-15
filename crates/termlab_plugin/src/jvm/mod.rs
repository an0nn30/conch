//! JVM plugin runtime — load and manage Java plugins via embedded JVM.
//!
//! Java plugins are JAR files that contain a class implementing
//! `termlab.plugin.TermLabPlugin`. The JAR manifest must include a
//! `Plugin-Class` entry pointing to the implementation class.
//!
//! Host functions are exposed to Java as static native methods on
//! `termlab.plugin.HostApi`, registered via JNI `RegisterNatives`.

pub mod runtime;
