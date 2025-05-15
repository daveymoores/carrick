const { Project } = require("ts-morph");

async function main() {
  const [, , fileA, typeA, fileB, typeB] = process.argv;

  const project = new Project({ tsConfigFilePath: "tsconfig.json" });

  const sourceA = project.getSourceFileOrThrow(fileA);
  const sourceB = project.getSourceFileOrThrow(fileB);

  const typeAliasA = sourceA.getTypeAliasOrThrow(typeA);
  const typeAliasB = sourceB.getTypeAliasOrThrow(typeB);

  const checker = project.getTypeChecker();
  const isAssignable = checker.isTypeAssignableTo(
    typeAliasA.getType(),
    typeAliasB.getType(),
  );

  console.log(JSON.stringify({ isAssignable }));
}

main();
