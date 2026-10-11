import {
  SERIAL_CHECKSUM_ALGORITHMS,
  serialChecksumWidth,
  type SerialChecksumConfig,
} from "@escode/shared/serial";
import { Input } from "@/components/ui/input.js";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";

/** 算法名是通用技术名称，不做翻译。 */
const ALGORITHM_LABELS: Record<SerialChecksumConfig["algorithm"], string> = {
  xor: "XOR",
  sum8: "SUM8",
  crc8: "CRC-8",
  "crc16-modbus": "CRC-16/MODBUS",
  "crc16-ccitt-false": "CRC-16/CCITT",
  crc32: "CRC-32",
  lrc: "LRC",
};

const NONE = "none";
const MAX_SKIP = 1024;

/**
 * 校验和配置（docs/specs/serial-port-debugger-phase3.md 第 5 节）：发送栏、快捷指令与接收帧校验共用。
 * value 为 undefined 表示不附加/不校验。
 */
export function SerialChecksumPicker({
  value,
  onChange,
  disabled,
  testId,
}: {
  value: SerialChecksumConfig | undefined;
  onChange: (value: SerialChecksumConfig | undefined) => void;
  disabled?: boolean;
  testId?: string;
}) {
  const { intl } = useESCodeIntl();
  const multiByte = value ? serialChecksumWidth(value.algorithm) > 1 : false;
  return (
    <div className="flex flex-wrap items-center gap-1 text-ui-sm text-foreground-subtle">
      <Select
        value={value?.algorithm ?? NONE}
        disabled={disabled}
        onValueChange={(algorithm) =>
          onChange(
            algorithm === NONE
              ? undefined
              : {
                  algorithm: algorithm as SerialChecksumConfig["algorithm"],
                  ...(value?.skip ? { skip: value.skip } : {}),
                },
          )
        }
      >
        <SelectTrigger
          size="sm"
          aria-label={intl.formatMessage({ id: "serial.checksum.label" })}
          data-testid={testId}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={NONE}>{intl.formatMessage({ id: "serial.checksum.none" })}</SelectItem>
          {SERIAL_CHECKSUM_ALGORITHMS.map((algorithm) => (
            <SelectItem key={algorithm} value={algorithm}>
              {ALGORITHM_LABELS[algorithm]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {value ? (
        <>
          <span>{intl.formatMessage({ id: "serial.checksum.skip" })}</span>
          <Input
            className="h-6 w-12 font-mono"
            inputMode="numeric"
            value={value.skip ? String(value.skip) : ""}
            placeholder="0"
            disabled={disabled}
            aria-label={intl.formatMessage({ id: "serial.checksum.skip" })}
            onChange={(event) => {
              const digits = event.target.value.replace(/\D/g, "");
              const skip = Math.min(MAX_SKIP, Number(digits || "0"));
              const { skip: _previous, ...rest } = value;
              onChange(skip > 0 ? { ...rest, skip } : rest);
            }}
          />
          {multiByte ? (
            <Select
              value={value.endian ?? "default"}
              disabled={disabled}
              onValueChange={(endian) => {
                const { endian: _previous, ...rest } = value;
                onChange(
                  endian === "default" ? rest : { ...rest, endian: endian as "big" | "little" },
                );
              }}
            >
              <SelectTrigger
                size="sm"
                aria-label={intl.formatMessage({ id: "serial.checksum.endian" })}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="default">
                  {intl.formatMessage({ id: "serial.checksum.endian.default" })}
                </SelectItem>
                <SelectItem value="big">
                  {intl.formatMessage({ id: "serial.checksum.endian.big" })}
                </SelectItem>
                <SelectItem value="little">
                  {intl.formatMessage({ id: "serial.checksum.endian.little" })}
                </SelectItem>
              </SelectContent>
            </Select>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
