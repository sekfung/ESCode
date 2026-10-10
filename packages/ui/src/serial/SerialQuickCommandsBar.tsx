import { useRef, useState } from "react";
import {
  closestCenter,
  DndContext,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  horizontalListSortingStrategy,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { EllipsisIcon, PencilIcon, PlusIcon, RepeatIcon, Trash2Icon } from "lucide-react";
import type { SerialQuickCommand } from "@zcode/shared";
import { Button } from "@/components/ui/button.js";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu.js";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover.js";
import { toast } from "@/components/ui/toast.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useZCodeIntl } from "@/i18n/IntlProvider.js";
import {
  addSerialQuickCommand,
  exportSerialQuickCommands,
  importSerialQuickCommands,
  moveSerialQuickCommand,
  removeSerialQuickCommand,
  updateSerialQuickCommand,
} from "@/lib/serial/serialQuickCommands.js";
import { SerialQuickCommandEditor } from "@/serial/SerialQuickCommandEditor.js";
import { cn } from "@/components/lib/utils.js";

/** 拖动超过该距离才开始排序，单击仍然是发送。 */
const DRAG_ACTIVATION_DISTANCE = 6;

function QuickCommandChip({
  command,
  canSend,
  onSend,
  onLoop,
  onEdit,
  onRemove,
}: {
  command: SerialQuickCommand;
  canSend: boolean;
  onSend: () => void;
  onLoop: () => void;
  onEdit: () => void;
  onRemove: () => void;
}) {
  const { intl } = useZCodeIntl();
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: command.id,
  });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn(
        "group/chip flex items-center rounded-lg border border-border",
        isDragging && "opacity-60",
      )}
      {...attributes}
      {...listeners}
    >
      <button
        type="button"
        disabled={!canSend}
        title={command.data}
        className="h-6 max-w-40 truncate px-2 text-ui-sm text-foreground hover:bg-hover disabled:text-foreground-subtlest"
        onClick={onSend}
        data-testid="serial-quick-command"
      >
        {command.name}
      </button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={intl.formatMessage({ id: "serial.quick.more" }, { name: command.name })}
            className="flex h-6 w-5 items-center justify-center text-foreground-subtle opacity-0 group-hover/chip:opacity-100 hover:bg-hover focus-visible:opacity-100"
          >
            <EllipsisIcon className="size-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem disabled={!canSend} onSelect={onLoop}>
            <RepeatIcon className="size-4" />
            {intl.formatMessage({ id: "serial.quick.loop" })}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onEdit}>
            <PencilIcon className="size-4" />
            {intl.formatMessage({ id: "serial.quick.edit" })}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={onRemove}>
            <Trash2Icon className="size-4" />
            {intl.formatMessage({ id: "serial.quick.delete" })}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

/**
 * 快捷指令栏（docs/specs/serial-port-debugger-phase3.md 第 3 节）：点击即发送到当前标签的串口，
 * 可拖拽排序、增删改、导入导出 JSON。列表是全局设置，由调用方持久化。
 */
export function SerialQuickCommandsBar({
  commands,
  canSend,
  onSend,
  onLoop,
  onChange,
}: {
  commands: readonly SerialQuickCommand[];
  canSend: boolean;
  onSend: (command: SerialQuickCommand) => void;
  /** 以当前发送栏的循环参数循环发送该指令。 */
  onLoop: (command: SerialQuickCommand) => void;
  onChange: (commands: SerialQuickCommand[]) => void;
}) {
  const { intl } = useZCodeIntl();
  const platform = usePlatform();
  const [editing, setEditing] = useState<SerialQuickCommand | "new" | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: DRAG_ACTIVATION_DISTANCE } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const handleImport = async (file: File) => {
    const result = importSerialQuickCommands(commands, await file.text());
    if (result.error) {
      toast(intl.formatMessage({ id: "serial.quick.importInvalid" }));
      return;
    }
    onChange(result.commands);
    toast(
      intl.formatMessage(
        { id: "serial.quick.imported" },
        { imported: result.imported, skipped: result.skipped, dropped: result.dropped },
      ),
    );
  };

  const handleExport = async () => {
    if (!platform.saveFile) return;
    const data = new TextEncoder().encode(exportSerialQuickCommands(commands));
    await platform.saveFile({
      data: data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength),
      suggestedName: "serial-quick-commands.json",
    });
  };

  return (
    <div className="flex flex-wrap items-center gap-1" data-testid="serial-quick-commands">
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={(event: DragEndEvent) => {
          if (event.over) {
            onChange(
              moveSerialQuickCommand(commands, String(event.active.id), String(event.over.id)),
            );
          }
        }}
      >
        <SortableContext
          items={commands.map((item) => item.id)}
          strategy={horizontalListSortingStrategy}
        >
          {commands.map((command) => (
            <QuickCommandChip
              key={command.id}
              command={command}
              canSend={canSend}
              onSend={() => onSend(command)}
              onLoop={() => onLoop(command)}
              onEdit={() => setEditing(command)}
              onRemove={() => onChange(removeSerialQuickCommand(commands, command.id))}
            />
          ))}
        </SortableContext>
      </DndContext>
      <Popover open={editing !== null} onOpenChange={(open) => !open && setEditing(null)}>
        <PopoverTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={intl.formatMessage({ id: "serial.quick.add" })}
            title={intl.formatMessage({ id: "serial.quick.add" })}
            onClick={() => setEditing("new")}
            data-testid="serial-quick-command-add"
          >
            <PlusIcon />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-auto">
          {editing ? (
            <SerialQuickCommandEditor
              key={editing === "new" ? "new" : editing.id}
              initial={editing === "new" ? undefined : editing}
              onCancel={() => setEditing(null)}
              onSave={(draft) => {
                onChange(
                  editing === "new"
                    ? addSerialQuickCommand(commands, draft)
                    : updateSerialQuickCommand(commands, editing.id, draft),
                );
                setEditing(null);
              }}
            />
          ) : null}
        </PopoverContent>
      </Popover>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={intl.formatMessage({ id: "serial.quick.manage" })}
            title={intl.formatMessage({ id: "serial.quick.manage" })}
          >
            <EllipsisIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem onSelect={() => fileInput.current?.click()}>
            {intl.formatMessage({ id: "serial.quick.import" })}
          </DropdownMenuItem>
          {platform.saveFile ? (
            <DropdownMenuItem disabled={commands.length === 0} onSelect={() => void handleExport()}>
              {intl.formatMessage({ id: "serial.quick.export" })}
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
      <input
        ref={fileInput}
        type="file"
        accept="application/json,.json"
        className="hidden"
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (file) void handleImport(file);
        }}
      />
    </div>
  );
}
