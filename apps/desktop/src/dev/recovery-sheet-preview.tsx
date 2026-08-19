import { RecoveryDialog } from "@/components/screens/recovery/recovery-dialog";

type RecoverySheetPreviewProps = {
  onOpenChange: (open: boolean) => void;
  open: boolean;
  portalContainer?: HTMLElement | null | undefined;
};

function RecoverySheetPreview(props: RecoverySheetPreviewProps) {
  return <RecoveryDialog {...props} onRecover={() => false} />;
}

export { RecoverySheetPreview };
