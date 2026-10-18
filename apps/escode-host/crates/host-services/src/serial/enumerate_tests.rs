//! 端口枚举（`rust-u2-serial.md` §3/§7）：假 sysfs 树。
use super::*;
use std::path::Path;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn usb_metadata_falls_back_to_the_device_parent() {
    let dir = tempfile::tempdir().unwrap();
    let sys = dir.path().join("sys");
    // ttyUSB0: device 是 USB 接口，元数据在上一级（USB 设备节点）。
    std::fs::create_dir_all(sys.join("ttyUSB0").join("device")).unwrap();
    write(&sys.join("ttyUSB0").join("idVendor"), "10c4\n");
    write(&sys.join("ttyUSB0").join("idProduct"), "ea60\n");
    write(&sys.join("ttyUSB0").join("manufacturer"), "Silicon Labs\n");
    write(&sys.join("ttyUSB0").join("serial"), "SN-1\n");
    // ttyS0: 有 device、无元数据。
    std::fs::create_dir_all(sys.join("ttyS0").join("device")).unwrap();
    // pts: 没有 device 子目录 → 虚拟 tty，跳过。
    std::fs::create_dir_all(sys.join("pts")).unwrap();
    // ttyACM0: 有 device，但 /dev 下没有节点 → 跳过。
    std::fs::create_dir_all(sys.join("ttyACM0").join("device")).unwrap();

    let dev = dir.path().join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    for name in ["ttyUSB0", "ttyS0"] {
        std::fs::write(dev.join(name), "").unwrap();
    }

    let ports = enumerate_sync(&sys, &dev).unwrap();
    let paths: Vec<&str> = ports
        .iter()
        .map(|port| port["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            dev.join("ttyS0").to_string_lossy().as_ref(),
            dev.join("ttyUSB0").to_string_lossy().as_ref()
        ],
        "按 path 排序；虚拟 tty 与缺 /dev 节点的都跳过"
    );
    let usb = &ports[1];
    assert_eq!(usb["manufacturer"], serde_json::json!("Silicon Labs"));
    assert_eq!(usb["serialNumber"], serde_json::json!("SN-1"));
    assert_eq!(usb["vendorId"], serde_json::json!("10c4"));
    assert_eq!(usb["productId"], serde_json::json!("ea60"));
    // 没有元数据的端口不输出这些键。
    let plain = &ports[0];
    assert_eq!(plain.len(), 1, "{plain:?}");
}

#[test]
fn ports_key_joins_paths_with_newline() {
    let ports = vec![port("/dev/a"), port("/dev/b")];
    assert_eq!(ports_key(&ports), "/dev/a\n/dev/b");
    assert_eq!(ports_key(&[]), "");
}

#[test]
fn missing_sys_root_is_an_error_not_an_empty_list() {
    let dir = tempfile::tempdir().unwrap();
    assert!(enumerate_sync(&dir.path().join("nope"), dir.path()).is_err());
}
