import { Input, type InputProps } from "@touchgrass/ui";

type DisplayNameInputProps = Omit<
  InputProps,
  "autoCapitalize" | "autoComplete" | "autoCorrect" | "spellCheck"
>;

function DisplayNameInput(props: DisplayNameInputProps) {
  return (
    <Input
      {...props}
      autoCapitalize="off"
      autoComplete="off"
      autoCorrect="off"
      spellCheck={false}
    />
  );
}

export { DisplayNameInput };
