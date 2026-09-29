import { useState } from 'react';
import { Check, ChevronsUpDown } from 'lucide-react';
import { Button } from './ui/button';
import { Popover, PopoverContent, PopoverTrigger } from './ui/popover';
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from './ui/command';
export function SearchChoice({ value, selectedLabel, options, onSelect, placeholder, searchLabel, emptyLabel }: { value: string; selectedLabel?: string; options: { value: string; label: string }[]; onSelect: (value: string, label: string) => void; placeholder: string; searchLabel: string; emptyLabel: string }) {
  const [open, setOpen] = useState(false);
  return <Popover open={open} onOpenChange={setOpen}><PopoverTrigger asChild><Button type="button" variant="outline" role="combobox" aria-expanded={open} aria-label={placeholder} className="w-full min-h-10 justify-between"><span className="truncate">{options.find(option => option.value === value)?.label || selectedLabel || placeholder}</span><ChevronsUpDown className="size-4 shrink-0 opacity-50" /></Button></PopoverTrigger><PopoverContent align="start" sideOffset={6} collisionPadding={12} className="w-[var(--radix-popover-trigger-width)] p-0"><Command><CommandInput placeholder={searchLabel} aria-label={searchLabel} /><CommandList><CommandEmpty>{emptyLabel}</CommandEmpty><CommandGroup>{options.map(option => <CommandItem key={option.value} value={option.value} keywords={[option.label]} onSelect={() => { onSelect(option.value, option.label); setOpen(false); }}><span className="truncate">{option.label}</span>{option.value === value && <Check className="ml-auto size-4 shrink-0" />}</CommandItem>)}</CommandGroup></CommandList></Command></PopoverContent></Popover>;
}
