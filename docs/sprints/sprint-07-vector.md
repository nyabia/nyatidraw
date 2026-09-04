# Sprint 7 - 벡터와 텍스트

## 사용자 결과

도형, path, 텍스트를 확대 가능한 layer로 만들고 raster layer와 함께 저장·합성·export
한다.

## 범위

1. backend-neutral `VectorScene`과 versioned wire schema를 확정한다.
2. vector layer, path/shape, text, transform과 raster composite를 구현한다.
3. Vello는 renderer adapter로만 사용하고 문서 포맷에 Vello 타입을 저장하지 않는다.
4. Vello 없이도 headless export가 가능한 reference path를 유지한다.

## 완료 gate

- raster/vector/text가 섞인 문서를 save/restart/reopen해 의미와 출력이 같다.
- renderer 교체 또는 GPU device 복구가 project schema를 바꾸지 않는다.
