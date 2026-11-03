import { randomBytes } from "node:crypto";
import forge from "node-forge";

const CA_ATTRS = [
  { name: "commonName", value: "ESCode Test CA" },
  { name: "organizationName", value: "ESCode" },
];

function serialNumber(): string {
  const serial = randomBytes(16);
  // 首字节清零，避免被 ASN.1 解析成负数。
  serial[0] = serial[0]! & 0x7f;
  return serial.toString("hex");
}

function validity(): { notBefore: Date; notAfter: Date } {
  const notBefore = new Date(Date.now() - 24 * 60 * 60 * 1000);
  const notAfter = new Date(notBefore);
  notAfter.setFullYear(notAfter.getFullYear() + 10);
  return { notBefore, notAfter };
}

/**
 * 测试用 CA→叶子证书链。使用 node-forge 而非宿主 openssl：macOS runner 自带 LibreSSL，其输出在
 * rustls 下与本用例的信任锚校验不兼容，会让同一个用例只在 macOS 失败。证书只用于 127.0.0.1。
 */
export function createCaSignedLeaf(): { caCert: string; key: string; cert: string } {
  const caKeys = forge.pki.rsa.generateKeyPair(2048);
  const ca = forge.pki.createCertificate();
  ca.publicKey = caKeys.publicKey;
  ca.serialNumber = serialNumber();
  ca.validity = validity();
  ca.setSubject(CA_ATTRS);
  ca.setIssuer(CA_ATTRS);
  ca.setExtensions([
    { name: "basicConstraints", cA: true, critical: true },
    { name: "keyUsage", critical: true, keyCertSign: true, cRLSign: true, digitalSignature: true },
    { name: "subjectKeyIdentifier" },
  ]);
  ca.sign(caKeys.privateKey, forge.md.sha256.create());

  const leafKeys = forge.pki.rsa.generateKeyPair(2048);
  const leaf = forge.pki.createCertificate();
  leaf.publicKey = leafKeys.publicKey;
  leaf.serialNumber = serialNumber();
  leaf.validity = validity();
  leaf.setSubject([{ name: "commonName", value: "127.0.0.1" }]);
  leaf.setIssuer(CA_ATTRS);
  leaf.setExtensions([
    { name: "basicConstraints", cA: false, critical: true },
    { name: "keyUsage", critical: true, digitalSignature: true, keyEncipherment: true },
    { name: "extKeyUsage", serverAuth: true },
    {
      name: "subjectAltName",
      altNames: [
        { type: 7, ip: "127.0.0.1" },
        { type: 2, value: "localhost" },
      ],
    },
  ]);
  leaf.sign(caKeys.privateKey, forge.md.sha256.create());

  return {
    caCert: forge.pki.certificateToPem(ca),
    key: forge.pki.privateKeyToPem(leafKeys.privateKey),
    cert: forge.pki.certificateToPem(leaf),
  };
}
