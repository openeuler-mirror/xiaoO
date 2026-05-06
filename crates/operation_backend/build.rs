fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("find vendored protoc");
    std::env::set_var("PROTOC", protoc);
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &["src/backends/conch/proto/agent.proto"],
            &["src/backends/conch/proto"],
        )
        .expect("compile conch agent proto");
}
