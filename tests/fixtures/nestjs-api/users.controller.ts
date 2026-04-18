import { Controller, Get, Post, Body, Param } from '@nestjs/common';

interface User {
  id: number;
  name: string;
}

interface CreateUserDto {
  name: string;
}

@Controller('users')
export class UsersController {
  @Get()
  findAll(): User[] {
    return [{ id: 1, name: 'Alice' }];
  }

  @Get(':id')
  findOne(@Param('id') id: string): User {
    return { id: Number(id), name: 'Alice' };
  }

  @Post()
  create(@Body() dto: CreateUserDto): User {
    return { id: 42, name: dto.name };
  }
}
