fn main() {
    println!("cargo:rerun-if-changed=../network/windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("../network/windows.manifest")
            .canonicalize()
            .unwrap();
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }

    println!("cargo:rustc-check-cfg=cfg(studio_windows_qos_compat)");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let windows_qos = target == "windows" && std::env::var("ROS_DISTRO").as_deref() != Ok("humble");
    println!("cargo:rerun-if-changed=native/service_shim.c");
    println!("cargo:rerun-if-changed=native/tf_shim.cpp");
    let mut tf = cc::Build::new();
    tf.cpp(true).std("c++17").file("native/tf_shim.cpp");
    if windows_qos {
        println!("cargo:rustc-cfg=studio_windows_qos_compat");
        println!("cargo:rerun-if-changed=native/windows_qos_compat.cpp");
        tf.file("native/windows_qos_compat.cpp");
    }
    let mut shim = cc::Build::new();
    if std::env::var("ROS_DISTRO").as_deref() == Ok("humble") {
        shim.define("STUDIO_ROS_HUMBLE", None);
    }
    shim.file("native/service_shim.c").file("native/qos_shim.c");
    println!("cargo:rerun-if-changed=native/qos_shim.c");
    if let Some(paths) = std::env::var_os("AMENT_PREFIX_PATH") {
        for prefix in std::env::split_paths(&paths) {
            let include = prefix.join("include");
            shim.include(&include);
            tf.include(&include);
            for package in [
                "tf2",
                "geometry_msgs",
                "builtin_interfaces",
                "std_msgs",
                "rosidl_runtime_cpp",
                "rosidl_buffer",
                "rosidl_runtime_c",
                "rosidl_typesupport_interface",
                "type_description_interfaces",
                "rcutils",
                "rmw",
            ] {
                tf.include(include.join(package));
            }
            for package in [
                "std_msgs",
                "rcl",
                "rcl_action",
                "action_msgs",
                "unique_identifier_msgs",
                "rcl_yaml_param_parser",
                "rcutils",
                "rmw",
                "rosidl_runtime_c",
                "rosidl_typesupport_interface",
                "rosidl_typesupport_introspection_c",
                "builtin_interfaces",
                "type_description_interfaces",
                "rcl_interfaces",
                "tracetools",
                "service_msgs",
                "rosidl_dynamic_typesupport",
            ] {
                shim.include(include.join(package));
            }
            println!(
                "cargo:rustc-link-search=native={}",
                prefix.join("lib").display()
            );
        }
    }
    shim.compile("studio_service_shim");
    tf.compile("studio_tf_shim");
    println!("cargo:rustc-link-lib=tf2");
    for lib in [
        "std_msgs__rosidl_typesupport_c",
        "rcl_action",
        "action_msgs__rosidl_typesupport_introspection_c",
        "rcl",
        "rcutils",
        "rosidl_runtime_c",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    println!("cargo:rerun-if-env-changed=ROS_DISTRO");
    let distro =
        std::env::var("ROS_DISTRO").expect("Build inside a distro-specific ROS environment");
    println!("cargo:rustc-env=STUDIO_ROS_DISTRO={distro}");
    println!("cargo:rerun-if-env-changed=AMENT_PREFIX_PATH");
    if target == "macos" || target == "linux" {
        if target == "macos" {
            // Each probe uses only part of the shared native boundary. Avoid
            // eagerly loading libraries whose symbols this executable never uses.
            println!("cargo:rustc-link-arg=-Wl,-dead_strip_dylibs");
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../lib");
        } else {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
        }
        if let Some(paths) = std::env::var_os("AMENT_PREFIX_PATH") {
            for path in std::env::split_paths(&paths) {
                println!(
                    "cargo:rustc-link-arg=-Wl,-rpath,{}",
                    path.join("lib").display()
                );
            }
        }
    }
}
