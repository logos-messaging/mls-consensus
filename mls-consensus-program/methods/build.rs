fn main() {
    // `cargo clippy` exports RUSTC_WORKSPACE_WRAPPER=clippy-driver, and the nested cargo that
    // `embed_methods` runs to cross-compile the guest inherits it. clippy-driver has no
    // sysroot for riscv32im-risc0-zkvm-elf, so the guest build dies with "can't find crate for
    // `std`" and takes `cargo clippy --workspace` down with it. The guest is still linted on
    // its own, by `cargo clippy -p mls_consensus_guest`.
    //
    // SAFETY: build scripts are single-threaded at this point; nothing else can be reading the
    // environment concurrently.
    unsafe {
        std::env::remove_var("RUSTC_WORKSPACE_WRAPPER");
    }

    risc0_build::embed_methods();
}
