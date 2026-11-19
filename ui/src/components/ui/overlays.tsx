import type { ComponentProps, ReactElement, ReactNode } from "react";
import { AlertDialog, DropdownMenu, Popover, Tooltip } from "radix-ui";
import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/*
 * Thin wrappers over Radix with WHY's look (classes live in index.css:
 * `.overlay-surface`, `.menu-item`, ...). Radix supplies focus handling,
 * keyboard support, portals and `data-state`, which the CSS animates.
 */

// ---- Tooltip ----------------------------------------------------------------

export function TooltipProvider({ children }: { children: ReactNode }) {
  // A short delay so passing over a control does not flash a tooltip; once one
  // is shown, neighbours open at once (`skipDelayDuration`).
  return (
    <Tooltip.Provider delayDuration={450} skipDelayDuration={250}>
      {children}
    </Tooltip.Provider>
  );
}

/** A tooltip on `children` (one focusable element). */
export function Tip({
  label,
  children,
  side = "bottom",
  shortcut,
}: {
  label: ReactNode;
  children: ReactElement;
  side?: ComponentProps<typeof Tooltip.Content>["side"];
  shortcut?: string;
}) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="tooltip" side={side} sideOffset={6} collisionPadding={8}>
          {label}
          {shortcut && <kbd>{shortcut}</kbd>}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

// ---- Popover ----------------------------------------------------------------

export const PopoverRoot = Popover.Root;
export const PopoverTrigger = Popover.Trigger;

export function PopoverContent({ className, ...props }: ComponentProps<typeof Popover.Content>) {
  return (
    <Popover.Portal>
      <Popover.Content
        sideOffset={6}
        collisionPadding={12}
        className={cn("overlay-surface popover", className)}
        {...props}
      />
    </Popover.Portal>
  );
}

// ---- Dropdown menu ----------------------------------------------------------

export const MenuRoot = DropdownMenu.Root;
export const MenuTrigger = DropdownMenu.Trigger;

export function MenuContent({ className, ...props }: ComponentProps<typeof DropdownMenu.Content>) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        sideOffset={4}
        align="end"
        collisionPadding={8}
        className={cn("overlay-surface menu", className)}
        {...props}
      />
    </DropdownMenu.Portal>
  );
}

export function MenuItem({
  className,
  danger,
  ...props
}: ComponentProps<typeof DropdownMenu.Item> & { danger?: boolean }) {
  return (
    <DropdownMenu.Item
      data-danger={danger || undefined}
      className={cn("menu-item", className)}
      {...props}
    />
  );
}

// ---- Alert dialog (confirmations) --------------------------------------------

export const ConfirmRoot = AlertDialog.Root;

export function ConfirmContent({
  title,
  description,
  cancelLabel = "Cancel",
  confirmLabel,
  onConfirm,
  onCloseFocus,
}: {
  title: string;
  description: ReactNode;
  cancelLabel?: string;
  confirmLabel: string;
  onConfirm: () => void;
  /**
   * Where focus goes when the dialog closes. Radix returns it to the element
   * that *triggered* the dialog, but one opened from code (here: from a menu
   * item that is already gone) has no trigger, so focus would be lost.
   */
  onCloseFocus?: () => void;
}) {
  return (
    <AlertDialog.Portal>
      <AlertDialog.Overlay className="modal-overlay" />
      <AlertDialog.Content
        className="overlay-surface modal confirm-dialog"
        onCloseAutoFocus={(event) => {
          if (!onCloseFocus) return;
          event.preventDefault();
          onCloseFocus();
        }}
      >
        <AlertDialog.Title>{title}</AlertDialog.Title>
        <AlertDialog.Description>{description}</AlertDialog.Description>
        <div className="modal-actions">
          {/* Cancel is the first focusable: Radix focuses it, so Enter never destroys by accident. */}
          <AlertDialog.Cancel className={buttonVariants({ variant: "ghost", size: "sm" })}>
            {cancelLabel}
          </AlertDialog.Cancel>
          <AlertDialog.Action
            className={buttonVariants({ variant: "destructive", size: "sm" })}
            onClick={onConfirm}
          >
            {confirmLabel}
          </AlertDialog.Action>
        </div>
      </AlertDialog.Content>
    </AlertDialog.Portal>
  );
}
