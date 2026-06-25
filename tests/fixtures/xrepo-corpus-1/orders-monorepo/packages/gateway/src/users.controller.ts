import { Controller, Get, Param } from '@nestjs/common';

// Shared user shape for this gateway.
export interface UserSummary {
  id: string;
  displayName: string;
}

// Trap: @Controller('users') + @Get(':id') — decorator prefix concat.
// The effective path is /users/:id, derived purely from decorators with NO
// synthetic mount (no app.use, no register call).  The scanner must concatenate
// the controller prefix and the method-level path to produce the full route.
@Controller('users')
export class UsersController {
  // GET /users/:id — producer-only (no consumer in corpus)
  // owner = findOne (the method name, resolved from the decorator)
  @Get(':id')
  findOne(@Param('id') id: string): UserSummary {
    return { id, displayName: 'placeholder' };
  }
}
