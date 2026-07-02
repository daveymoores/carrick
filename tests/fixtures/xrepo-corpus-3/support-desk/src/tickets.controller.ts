import { Controller, Get, Param } from "@nestjs/common";

@Controller("tickets")
export class TicketsController {
  // No return annotation on purpose: the response shape must be inferred
  // (the corpus's Implicit type-state case).
  @Get(":id")
  async findOne(@Param("id") id: string) {
    return {
      id,
      subject: "Cracked mug on arrival",
      ageDays: 3,
    };
  }
}
