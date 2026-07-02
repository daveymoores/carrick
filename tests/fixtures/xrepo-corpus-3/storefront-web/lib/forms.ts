import { z } from "zod";

// Client-side form validation — zod 4 in real use (the dep-conflict vehicle:
// the monorepo and inventory-svc pin zod 3).
export const CheckoutFormSchema = z.object({
  email: z.string(),
  promoCode: z.string(),
});

export type CheckoutForm = z.infer<typeof CheckoutFormSchema>;

export function validateCheckoutForm(input: unknown): CheckoutForm {
  return CheckoutFormSchema.parse(input);
}
