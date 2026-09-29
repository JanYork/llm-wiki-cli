import type { ReactNode } from 'react';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
const EMPTY = '__lwc_empty_selection__';
export function Choice({ value, onValueChange, children, disabled, required, label }: { value: string; onValueChange: (value: string) => void; children: ReactNode; disabled?: boolean; required?: boolean; label?: string }) {
  return <Select value={value || EMPTY} onValueChange={next => onValueChange(next === EMPTY ? '' : next)} disabled={disabled} required={required}>
    <SelectTrigger className="w-full min-h-10" aria-label={label}><SelectValue /></SelectTrigger>
    <SelectContent position="popper" align="start" sideOffset={4} collisionPadding={12} className="w-[var(--radix-select-trigger-width)] min-w-[var(--radix-select-trigger-width)] max-h-80">{children}</SelectContent>
  </Select>;
}
export function ChoiceOption({ value = '', disabled, children }: { value?: string; disabled?: boolean; children?: ReactNode }) {
  return <SelectItem value={value || EMPTY} disabled={disabled}>{children}</SelectItem>;
}
